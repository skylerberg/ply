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

`hash_module` and `hash_ast` hash **one module with no program around it**, so
they have no namespace to resolve an import against. A module that declares one
is therefore refused with `UNKNOWN_MODULE` rather than hashed. Accepting it would
write the binder the file happened to spell where the referent's hash belongs,
which aliases two modules imported under one name, aliases two selective imports
of one name, and makes `as`-renaming an import move a hash — the last
contradicting required test 4 above. Anything that imports goes through
`hash_program`.

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
`pub const &str` in this crate) invalidates everything. The **result** cache is
JSON on disk and stays that way: it is small, and `cat .ply-cache/results.json`
answering "why didn't this test re-run" is worth more than its parse cost. It is
small only because the pass records live beside it rather than in it — see Cache
storage below, which is the authority. The
**front-end** cache is not JSON — see Cache storage below. Writes must be atomic
(temp file + rename) so an interrupted run cannot corrupt the cache. A corrupt or
unreadable cache must degrade to an empty cache with a warning, never a crash.

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
    pub fn fingerprint(&self, path: &Path) -> Option<Arc<SourceFingerprint>>;
    pub fn put_source(&mut self, path: &Path, f: SourceFingerprint) -> bool;
    pub fn forget_source(&mut self, path: &Path) -> bool;
    pub fn source_paths(&self) -> Vec<PathBuf>;
    pub fn def(&self, hash: DefHash) -> Option<Arc<CachedDef>>;
    pub fn def_of(&self, hash: DefHash, name: &Symbol) -> Option<Arc<CachedDef>>;
    pub fn put_def(&mut self, hash: DefHash, def: CachedDef);
    pub fn decl(&self, hash: DefHash) -> Option<Arc<CachedDecl>>;
    pub fn decl_of(&self, hash: DefHash, name: &Symbol) -> Option<Arc<CachedDecl>>;
    pub fn put_decl(&mut self, hash: DefHash, decl: CachedDecl);
    pub fn prune(&mut self, keep: &[PathBuf]) -> Pruned;
    pub fn sources_len(&self) -> usize;
    pub fn defs_len(&self) -> usize;
    pub fn decls_len(&self) -> usize;
    pub fn frontend_is_empty(&self) -> bool;
    /// The front-end cache's index; `frontend_data_path` is the data file
    /// beside it. Both live under `Store::dir`.
    pub fn frontend_path(&self) -> &Path;
    pub fn frontend_data_path(&self) -> &Path;
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
- `CheckOutput`'s `defs`, `effects`, `ctors` and `modules` are ordered by the
  run's files and, within a file, by source position — never by the order
  inference happened to visit them in. Inference walks modules dependency-first
  and does not walk a skipped one at all, so a check-ordered map lists a project
  differently depending on what the cache held, and two `ply check --types` runs
  over one unchanged tree stop diffing against each other.
- `ImportEdge::exports` is a gate 1 refusal condition, not decoration: a digest
  that no longer matches refuses the skip. It is the only condition that sees a
  name a file imports **without using**, which is in no `deps` entry, so
  deleting or renaming such a name downstream would otherwise leave a dangling
  import unreported. The digest is over every top-level `(name, DefHash)` pair,
  including private ones — a `DefEntry` carries no visibility, so a skipped
  module's side of the comparison cannot be filtered and neither may the other.
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

## Cache storage

`docs/adr/0003-cache-storage.md` has the reasoning and the measurements; this
section is the contract. **Where it disagrees with the two sections above, this
section wins** — it was written after them.

The result cache stays JSON. The front-end cache becomes a binary
content-addressed store: `.ply-cache/frontend.idx`, a small index rewritten
whole and atomically, over `.ply-cache/frontend.dat`, an append-only data file
that is mmap'd and whose entries are decoded on demand. `Store::open` must be
under **5 ms at 10,000 definitions** and must decode no entry.

That budget covers the *whole* of `Store::open`, both caches. The result cache is
therefore split across two JSON files, because its two halves have opposite
read profiles:

| file | holds | read |
| --- | --- | --- |
| `results.json` | `DefHash -> Outcome`, and the definitions a run has seen | whole, at `Store::open` |
| `passes.json` | `PassRecord` per test key | only when something asks for a baseline — that is, only when a test failed |

A pass record is a whole closure, so at 10,000 definitions the records are seven
of the result cache's nine megabytes while a green run never looks at one.
`Store::open` must not read `passes.json`, and no accessor other than
`pass_record`, `pass_records_len` and `prune`'s baseline roots may force it.
`prune` must ask `Frontend::prune_would_change` before gathering those roots, or
a run with nothing to prune pays for the file anyway.

Both files carry `format` and `runtime_version`. `results.json` format 1 held the
pass records inline; it is still read, its records are relocated by the first
flush, and it is rewritten as format 2. **Nothing is discarded and no test
re-runs** — which is why this is a format bump and not a `RUNTIME_VERSION` bump.
`passes.json` is written before `results.json` in a flush, so the inline copy is
never dropped before the records exist somewhere else.

### Constants and types

The files are `frontend.idx` and `frontend.dat` under `Store::dir`, reachable as
`frontend_path` and `frontend_data_path`.

```rust
/// The on-disk generation of the front-end cache, in both file headers.
pub const FRONTEND_FORMAT: u32;

/// Covers every stored shape: `Type`, `Scheme`, `Footprint`, the fingerprint's
/// fields, the declaration bodies, and `BODY_ENCODING`. Written into both file
/// headers; a mismatch discards the front-end cache.
pub fn schema_fingerprint() -> ContentHash;

/// Version of the definition-body encoding, which lives in `ply-hash`.
pub const BODY_ENCODING: u32;

/// One definition's canonical body bytes, keyed by its `DefHash`.
pub struct DefBody { .. }
impl DefBody {
    pub fn new(encoding: u32, bytes: Vec<u8>) -> DefBody;
    pub fn encoding(&self) -> u32;
    pub fn as_bytes(&self) -> &[u8];
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}

impl Store {
    pub fn body(&self, hash: DefHash) -> Option<Arc<DefBody>>;
    pub fn put_body(&mut self, hash: DefHash, body: DefBody);
    pub fn has_body(&self, hash: DefHash) -> bool;
    pub fn bodies_len(&self) -> usize;

    pub fn stats(&self) -> CacheStats;
    /// Same precondition as `prune`: a run that discovered every `.ply` file.
    pub fn compact(&mut self, keep: &[PathBuf]) -> anyhow::Result<Compaction>;
    /// A program-wide name, a simple name, or a hash prefix of >= 4 hex chars.
    pub fn lookup(&self, query: &str) -> Vec<Found>;
}

pub struct CacheStats {
    pub results: usize,
    pub definitions_seen: usize,
    pub sources: usize,
    pub defs: usize,
    pub decls: usize,
    pub bodies: usize,
    /// `results.json` and `passes.json` together.
    pub results_bytes: u64,
    pub index_bytes: u64,
    pub data_bytes: u64,
    /// What `compact` would reclaim. `None` while the front end is one JSON
    /// document, which has no unreachable region to measure.
    pub garbage_bytes: Option<u64>,
}

pub struct Compaction {
    pub dropped: Pruned,          // `Pruned` gained a `bodies` count
    pub bytes_before: u64,
    pub bytes_after: u64,
}

pub enum Found { Def(FoundDef), Test(FoundTest) }
pub struct FoundDef {
    pub hash: DefHash, pub name: Symbol, pub kind: DefKind,
    pub path: PathBuf, pub span: FileSpan,
}
pub struct FoundTest {
    pub hash: DefHash, pub name: String, pub nondet: bool,
    pub footprint: Footprint, pub path: PathBuf, pub span: FileSpan,
}
```

### Definition bodies

`bodies: DefHash -> DefBody` is the third element DESIGN.md §3 promises and only
Type and Footprint ever delivered. M5's bisection has to *evaluate* a historical
definition set, which its types alone cannot do.

The encoding lives in `ply-hash`, beside the normalizer whose byte stream it is.
Required properties, each of which is testable:

1. The bytes are a function of the `DefHash`: names, spans and module membership
   never enter them, locals are de Bruijn levels, and a free reference is the
   referent's hash.
2. They are self-checking against their own key. For a definition that is its own
   component `blake3(bytes)` *is* the key; for a member of a recursive component
   the bytes are the component's and the key is `blake3(bytes ‖ index_le_u32)`.
   `put_body` refuses a body that does not verify, and warns.
3. Decoding yields an *evaluable* definition, not the user's source: names are
   synthesized, spans come back `Span::DUMMY`, and the reconstituted program is
   resolved from its own synthesized namespace.
4. A decoded definition is equal to the original only up to normalization's
   semantics-preserving rewrites — reordered commutable `let`s, sorted effect
   operations, sorted record *type* fields, a dropped `{ e }` wrapper.

### Versioning

Three gates, because a non-self-describing encoding that misparses produces a
wrong *footprint*, and footprints decide which tests may run concurrently:

- **Compile time.** `ply_store::schema` names every variant of every stored enum
  through exhaustive `match`es with no wildcard arm, so adding a variant does not
  compile until it is named, and a coverage test then fails until an exemplar
  value mentions it.
- **Test time.** A pin test compares `schema_fingerprint()` against a constant,
  prints the new digest and says to bump `FRONTEND_VERSION`.
- **Run time.** Both file headers carry a magic number, `FRONTEND_FORMAT`,
  `schema_fingerprint()`, `blake3(FRONTEND_VERSION)` and a shared nonce pairing
  the two files. Any mismatch degrades to an empty front-end cache with a
  warning. Below that, every entry carries a length prefix and a checksum, so a
  torn append is detected per entry. **A mismatch must never misparse.**

### Writing, compaction, migration

- Readers take no lock: an append never moves a byte another process has mapped.
- Writers must hold the cache lock. A writer that cannot take it within the
  bounded wait **writes nothing and warns** — interleaved frames are corruption,
  where a lost update is only a recheck. This replaces today's best-effort lock.
- A flush re-reads the index under the lock, truncates the data file to the
  `data_len` that index records (recovering a torn tail), appends, `fsync`s the
  data file, then writes the index atomically. The data must be durable before
  the index that names it.
- An entry is unreachable when nothing in the pruned index names it: a
  fingerprint for a path that no longer exists, an interface or body whose
  `DefHash` no surviving fingerprint declares, or any superseded record.
  `compact` copies the reachable records into a fresh data file and index under
  the lock. It never runs automatically; `ply cache stats` reports the garbage
  ratio and suggests it past 50%.
- **Migration.** There is no reader for the JSON front-end cache. A `Store::open`
  that finds `frontend.json` and no `frontend.idx` warns (`W0603`) that the
  format changed, that this run recomputes types and hashes for the whole
  project, and that the *result* cache is untouched so no test re-runs; it starts
  from an empty front-end cache and deletes `frontend.json` at the next
  successful flush.

### Transitional

`cached_def`, `cached_def_of`, `cached_decl`, `cached_decl_of` and `source`
return borrowed entries, which a store that materializes an entry from a mapped
byte range cannot do. `def`, `def_of`, `decl`, `decl_of` and `fingerprint` are
their replacements and are the contract. The borrowed forms remain until the
mmap lands and are removed by the same change, which has to touch their call
sites anyway. Do not add callers.

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
ply cache clear|stats|compact|inspect <def>
```

`ply cache stats` reports both caches' entry counts and bytes, and the front-end
data file's garbage ratio, suggesting `compact` past 50% — which is also what a
dry run would have printed, so there is no second code path for one.
`ply cache compact` must discover every `.ply` file under the root before it
drops anything. `ply cache inspect <def>` takes a program-wide name, a simple
name, or a hash prefix of at least four hex characters, and prints per match the
hash, kind, declaring file, resolved type, footprint, resolution witness, whether
a body is stored, and — for a test — its cached outcome. Several matches print
several entries; no match is `E0101`. All four take `--json`.

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

---

## Machine-shaped failure

`docs/adr/0004-machine-shaped-failure.md` has the reasoning; this section is the
contract. **Where it disagrees with the `ply-test` section above, this section
wins** — it was landed after it.

### The rule everything else follows from

A failure artifact answers, in this order: which change caused this, what
actually ran, what else could have, and what was asserted. The terminal output
follows the same order, because the culprit is the answer and the diff is the
evidence. A field an agent cannot act on does not go in the artifact.

### `Failure` — landed in `ply-test`

```rust
pub struct Failure {
    pub name: String,
    pub key: Symbol,
    pub diagnostic: Diagnostic,
    /// Unchanged: the raw closure ∩ changed intersection, by name.
    pub suspects: Vec<Symbol>,
    /// `None` until `ply-eval` carries the payload instead of rendering it into
    /// the diagnostic's notes.
    pub assertion: Option<Assertion>,
    pub attribution: Attribution,
}

pub struct Attribution {
    /// The same *set* as `Failure::suspects`, ranked and annotated. The order
    /// differs deliberately.
    pub suspects: Vec<Suspect>,
    pub bisection: Bisection,
    pub slice: Option<CausalSlice>,
}
impl Attribution {
    pub fn from_suspects(names: &[Symbol], hashes: &HashOutput) -> Attribution;
    pub fn resolve(&mut self, bisection: Bisection, slice: Option<CausalSlice>);
    pub fn culprits(&self) -> Vec<Symbol>;
}

pub struct Suspect {
    pub name: Symbol,
    pub hash: Option<DefHash>,
    pub before: Option<DefHash>,        // at the last pass; None with no baseline
    pub change: Option<ChangeKind>,     // None when the two were never compared
    pub ran: Option<bool>,              // None when the failure was not traced
    pub depth: Option<usize>,           // 0 is the failing frame
    pub culprit: bool,
}
```

Every judgement is tri-state where the evidence may be missing. A consumer that
cannot tell "did not run" from "was not traced" acts on the wrong one.

**Ranking**, total and deterministic: bisected culprit; on the failing stack,
innermost first; ran but had returned; untraced; did not run — ties broken toward
an edit over an inherited hash, then by name. Two runs over one failure must
produce byte-identical artifacts.

### Bisection — landed in `ply-test::bisect`

```rust
pub enum ChangeKind { Edited, Derived, Added, Removed }
pub struct Change {
    pub name: Symbol,
    pub before: Option<DefHash>, pub after: Option<DefHash>,
    pub kind: ChangeKind,
    /// Its published interface — canonicalized scheme and footprint — is the
    /// same on both sides.
    pub independent: bool,
}
impl Change {
    pub fn edited(name: Symbol, before: DefHash, after: DefHash, independent: bool) -> Change;
    pub fn derived(name: Symbol, before: DefHash, after: DefHash) -> Change;
    pub fn added(name: Symbol, after: DefHash) -> Change;    // never independent
    pub fn removed(name: Symbol, before: DefHash) -> Change; // never independent
}

pub struct DepEdges { .. }               // unioned over BOTH configurations
impl DepEdges {
    pub fn new() -> DepEdges;
    pub fn add(&mut self, from: Symbol, to: Symbol);
    pub fn extend_from_hashes(&mut self, hashes: &HashOutput);
    pub fn referrers(&self, name: &Symbol) -> impl Iterator<Item = &Symbol>;
}

pub enum FusionReason { Independent, InterfaceChanged, Existence }
pub struct Cluster { pub members: Vec<Symbol>, pub reason: FusionReason }

pub struct Delta { pub test: Option<Change>, pub changes: Vec<Change>,
                   pub clusters: Vec<Cluster> }
impl Delta {
    pub fn new(test: Option<Change>, changes: Vec<Change>, edges: &DepEdges) -> Delta;
    pub fn candidates(&self) -> usize;
    pub fn flipped_names(&self, flipped: &[usize]) -> Vec<Symbol>;
}

pub enum Unresolved { DoesNotCheck, DifferentFailure, MissingBody, BudgetSpent }
pub enum TrialOutcome { Fails, Passes, Unresolved(Unresolved) }
pub struct Trial { pub outcome: TrialOutcome, pub cached: bool }

pub trait Hybrid {
    fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial;
}

pub struct Budget { pub max_trials: usize }   // DEFAULT = 64, in evaluations
pub struct SearchStats {
    pub candidates: usize, pub clusters: usize,
    pub evaluated: usize, pub cached: usize, pub memoized: usize,
    pub unresolved: usize, pub exhausted: bool,
}

pub enum Skipped { NotRequested, NeverPassed, Nondet, Panicked, NoChanges, NoBodies }
pub enum Verdict { Bisected, Sole, TestChanged, NotInTheGraph, NotReproduced,
                   Inconclusive, NotAttempted(Skipped) }
pub enum Confidence { Minimal, Fused, Partial, None }

pub struct Bisection {
    pub verdict: Verdict, pub confidence: Confidence,
    pub groups: Vec<Vec<Symbol>>, pub reason: String, pub search: SearchStats,
}
impl Bisection {
    pub fn not_attempted(why: Skipped) -> Bisection;
    pub fn culprits(&self) -> Vec<Symbol>;
    pub fn is_conclusive(&self) -> bool;
}

pub fn bisect(delta: &Delta, hybrid: &mut dyn Hybrid, budget: Budget) -> Bisection;
```

Requirements, in the order a defect in them costs most:

- **`Derived` is decided by re-normalizing the current body against the baseline
  hash table**, not by comparing name sets or interfaces. Both cheaper tests are
  unsound, and a false `Derived` drops a real candidate and yields a confidently
  wrong culprit.
- **Fusion rule**: a candidate whose interface is unchanged stands alone; one
  whose interface changed is fused with every candidate that mentions it.
  `Added`/`Removed` are never independent. An unavailable baseline interface
  means `independent: false` — conservative in the safe direction.
- **`DepEdges` must union both eras.** The current graph misses a baseline body
  referencing a since-deleted definition; the baseline graph misses a caller
  written against a since-added one.
- Every hybrid runs the test **as it is written now**. `H(∅)` failing is an
  answer: `TestChanged` if the test was edited, `NotInTheGraph` if it was not.
- A hybrid that does not typecheck is `Unresolved`, never a failure. **Any**
  unresolved trial caps confidence at `Partial` — the search walked around a
  question it could not ask. A search that narrows *nothing* while hitting
  unresolved trials is `Inconclusive`, not `Bisected` over the whole set.
- A hybrid that passes may be written to the result cache as `Pass` under its own
  test hash. Bisection must **never** call `observe_definitions`: a definition
  proved fine in a hybrid has not been vindicated in the real program, and
  recording it would empty the next run's suspect set.
- A cached trial costs nothing and is not charged against the budget. The budget
  is in evaluations, never in seconds — an artifact that varies with machine load
  cannot be diffed against yesterday's.

### Causal slice — landed in `ply-test::slice`

```rust
pub struct CausalSlice {
    pub traced: bool,
    pub reproduced: bool,
    pub entered: Vec<Entered>,     // first-entry order
    pub stack: Vec<Frame>,         // outermost first; the last frame failed
    pub observed: Footprint,       // atoms actually performed
    pub truncated: bool,
}
pub struct Entered { pub name: Symbol, pub hash: Option<DefHash>, pub calls: u32 }
pub struct Frame   { pub name: Symbol, pub hash: Option<DefHash>, pub call_site: Span }
impl CausalSlice {
    pub fn untraced() -> CausalSlice;
    pub fn ran(&self, name: &Symbol) -> bool;
    pub fn depth_of(&self, name: &Symbol) -> Option<usize>;
    pub fn path(&self) -> Vec<&Symbol>;
}

pub enum AssertionKind { Eq, Bool, Panic, Runtime, UnhandledEffect, RecursionLimit }
pub struct Difference { pub path: String, pub expected: String, pub actual: String }
pub struct Assertion {
    pub kind: AssertionKind,
    pub expected: Option<String>, pub actual: Option<String>,
    pub first_difference: Option<Difference>,
    pub message: Option<String>,
}
```

Tracing happens on a **re-run**, not on the first execution: the green path is
the one that must be fast, and a `det` test replays identically by construction.
The traced re-run doubles as the reproduction check — a `det` test that passes on
replay sets `reproduced: false` and is reported rather than bisected.

Everything on the stack ran; not everything that ran is on the stack. A
definition that returned before the assertion has `ran: true` and `depth: None`.

### Required of other crates — not yet implemented

```rust
// ply-store — the baseline. One record per test key, overwritten on each pass,
// never written for a failing or nondet test.
pub struct PassRecord {
    pub test_hash: DefHash,
    pub closure: BTreeMap<Symbol, DefHash>,
}
impl Store {
    pub fn pass_record(&self, key: &Symbol) -> Option<&PassRecord>;
    pub fn put_pass_record(&mut self, key: Symbol, record: PassRecord);
    /// Bodies, per ADR 0003 — name-erased and hash-linked, so a hybrid mixing
    /// two namespaces needs no module layout invented for it.
    pub fn body(&self, hash: DefHash) -> Option<&Definition>;
}
```

`prune` gains a second retention root: every hash reachable from a surviving
`PassRecord::closure`. Dropping those silently downgrades every future bisection
to `no_bodies`.

`ply-eval` gains the tracer — hooked at `Interp::apply` for named closures and at
the perform site for atoms, both of which already hold the qualified name — and a
structured `Assertion` payload alongside the diagnostic. `ply-core` and `ply-eval`
must both accept a hash-linked definition graph so a hybrid can be checked and
run without being re-resolved.

### `ply test` flags and JSON

```
--bisect <auto|always|never>   default auto
--bisect-budget <n>            hybrid evaluations, default 64
--trace <auto|always|never>    default auto: trace a failing test's re-run
```

`auto` bisects when the test failed and did not panic, is not `test/nondet`, has
a `PassRecord`, has a non-empty delta, and has bodies. `always` ignores the
budget and still respects those preconditions — none can be waived without
inventing evidence. A single-cluster delta produces a verdict having evaluated
nothing, which is the common case and why the default is on.

Bisections are scheduled through `group_by_conflict` over the failing tests'
footprints: a hybrid performs the same effects the real test does.

`ply test --json` carries `"schema_version": 2` at the top level and the failure
artifact of ADR 0004 §7. `failures[].suspects` becoming an array of objects is a
breaking change against v1, ranked so a consumer reading only `suspects[0]` gets
the best guess. `ply-test`'s own `report::failure_json` is the reference shape;
`ply-cli` adds `location`, `module`, `test_hash` and `footprint` from the
`SourceMap` and `CheckOutput` it holds and `ply-test` does not.

### Required tests

1. One edited definition → named exactly, `confidence: minimal`, **zero**
   hybrids evaluated.
2. Five edits, one culprit → named exactly, in at most `2·log₂(5)` evaluations.
3. Two edits that only fail together → both named, neither alone.
4. A signature change and its caller → one fused cluster, no hybrid splitting
   them is ever built, `confidence: fused`.
5. A `Derived` change is never a candidate and sorts below every edited suspect.
6. A test that never passed → `not_attempted`/`never_passed`, zero hybrids, and
   still a causal slice.
7. `test/nondet` → `not_attempted`/`nondet`. 8. A panic →
   `not_attempted`/`panicked`.
9. Editing only the test body → `test_changed` naming the test.
10. The baseline reproduces it and the test is unedited → `not_in_the_graph`.
11. A hybrid that does not typecheck is `unresolved`; the search completes and
    confidence drops to `partial`.
12. Bisection never calls `observe_definitions`: the next run's suspect set is
    unchanged.
13. A hybrid already in the result cache is answered without evaluating and is
    not charged against the budget.
14. A spent budget → `exhausted: true`, `partial`, and the true cause still in
    the set.
15. The causal slice names only definitions that ran; its stack ends at the
    failing frame.
16. A definition that returned before the assertion has `ran: true`,
    `depth: null`.
17. Two runs over one failure produce byte-identical artifacts, including the
    order of `suspects` and `culprit.groups`.
18. `--bisect never` → `not_attempted`/`not_requested`, nothing evaluated.
19. The summary prints the culprit above the assertion when one exists, and no
    culprit line when none does.
20. A search that narrows nothing while hitting unresolved trials reports
    `Inconclusive`, not `Bisected` over the whole change set.

### Not in M5

Shrinking input **values** is property-test territory and belongs to **M8**:
`test` takes no parameters, so there is nothing to shrink. Do not build a value
shrinker speculatively. Also deferred: bisecting across git history, forking a
fixture per hybrid (M6), a seed as the repro artifact (M7), and suggesting a fix.
