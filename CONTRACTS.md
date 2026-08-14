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
    /// Ply's fault rather than the program's, so there is nothing in the
    /// definition graph to attribute it to. Observed by the run — the evaluator
    /// unwound, or it reported `INTERNAL_ERROR` — and never inferred from
    /// `RUNTIME_ERROR`, which a program reaches legitimately.
    pub defect: bool,
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

---

## The control stack and the world

`docs/adr/0005-control-stack-and-world.md` has the reasoning; this section is
the contract. **Where it disagrees with any section above, this section wins** —
it was landed after them.

### The rule everything else follows from

> State is a value the machine threads. Control is a value the machine splices.
> A continuation captures control only.

A resumption therefore observes the world **as of the handler's call to
`resume`**, never as of the capture. There is exactly one current world at every
point of an execution and it moves forward. Snapshot-at-capture makes a
cell-backed state handler unwritable — the clause's own write would be discarded
before the computation that asked for it ran — and the ADR's §3 is the argument.

The world is **monotone**: an entry is never removed. That is what makes a
`CellId` unable to dangle, and it is what lets a continuation captured inside a
`with_cell` region be resumed outside it and read the cell successfully rather
than being forbidden.

### `Value` — landed in `ply-eval`

```rust
pub enum Value {
    Int(i64), Bool(bool), Str(Arc<str>), Unit,
    List(Vector<Value>),
    Record(Arc<BTreeMap<Symbol, Value>>),
    Ctor { name: Symbol, args: Arc<Vec<Value>> },
    Closure(Arc<Closure>),
    /// A key into the `World`, not a pointer into it.
    Cell(CellId),
    /// Callable with exactly one argument — the value the `perform` it was
    /// captured at should have produced. Any other count is `ARITY_MISMATCH`.
    Continuation(Rc<Continuation>),
}

impl Value {
    pub fn as_cell(&self, span: Span, what: &str) -> Result<CellId, Diagnostic>;
}
```

`Cell(Rc<RefCell<Value>>)` is gone, and with it the "cell cannot be read while it
is already borrowed" runtime error — a `RefCell` reentrancy failure cannot happen
to a map entry. `values_equal` compares cells by `CellId`, which is the same
identity comparison `Rc::ptr_eq` gave. A `Continuation` compares like a closure:
`RUNTIME_ERROR`, "cannot compare functions for equality".

### `ply-eval::world` — landed

```rust
pub struct CellId(pub u32);          // Display: "#7"

pub struct World { .. }              // Clone, Default, Debug
impl World {
    pub fn new() -> World;
    /// O(1); structural sharing. The whole of "fork a fixture per test".
    pub fn fork(&self) -> World;
    pub fn alloc(&mut self, initial: Value) -> CellId;
    pub fn get(&self, id: CellId) -> Option<&Value>;
    /// `false` when the id is not in this world; the caller must report it.
    pub fn set(&mut self, id: CellId, value: Value) -> bool;
    pub fn with(&self, id: CellId, value: Value) -> World;
    pub fn contains(&self, id: CellId) -> bool;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    /// Ascending by id, so two runs iterate identically.
    pub fn cells(&self) -> impl Iterator<Item = (CellId, &Value)>;
    pub fn high_water(&self) -> u32;
}
```

Two worlds forked from one ancestor agree on every id below the ancestor's
high-water mark and may hand out the **same** id for different cells above it.
That is sound only because the machine holds one world at a time and no operation
carries a value from one fork into a sibling. Do not "fix" this into a global
counter; a landed test asserts the collision.

Handler-backed resources need no second map. A handler is a closure, its state is
a cell, and the cell is in the world — so a fixture is a `(World, Value)` pair
and forking it is `World::fork` plus a `Value` clone.

### `ply-eval::code` — landed

The AST lowered once per machine, with `Rc` on every node, so a frame can hold a
subexpression without a lifetime on `Value` and without cloning a subtree per
push. The shape mirrors `ExprKind` one to one; patterns, names, literals and
operators are reused from the AST.

```rust
pub type Code = Rc<Node>;
pub struct Node { pub kind: NodeKind, pub span: Span }
pub enum NodeKind { Lit, Var, Unary, Binary, Lambda, App, If, Match, Block,
                    Record, Field, List, Perform, Handle, WithCell }
pub struct Arm { pub pat: Pattern, pub guard: Option<Code>, pub body: Code, pub span: Span }
pub enum Stmt { Let { pat: Pattern, value: Code, span: Span }, Expr(Code) }
pub struct Clause {
    pub effect: QName, pub op: Symbol, pub resource: Option<Symbol>,
    pub params: Rc<Vec<Symbol>>,
    /// `None` is the tail-resumptive form, which needs no capture.
    pub resume: Option<Symbol>,
    pub body: Code, pub span: Span,
}
pub struct ReturnArm { pub binder: Symbol, pub body: Code, pub span: Span }

pub fn lower(e: &Expr) -> Code;
```

`lower_clause` sets `resume: None` today and must read `HandleClause::resume` as
soon as the grammar below lands. It is the only place that has to learn about it.

### `ply-eval::cont` — landed

```rust
pub enum Frame { .. }                // 20 kinds; see the ADR's table
pub struct Prompt {
    pub clauses: Rc<Vec<Clause>>,
    /// Program-wide effect names, parallel to `clauses`, resolved where the
    /// `handle` was written.
    pub effects: Rc<Vec<Symbol>>,
    pub ret: Option<Rc<ReturnArm>>,
    pub env: Env, pub module: usize, pub span: Span,
}
impl Prompt {
    pub fn clause_for(&self, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>)
        -> Option<usize>;
}

pub struct Segment { .. }
impl Segment {
    pub fn base() -> Segment;
    pub fn under(prompt: Rc<Prompt>) -> Segment;
    pub fn prompt(&self) -> Option<&Rc<Prompt>>;
    pub fn frames(&self) -> usize;
}

pub enum Next { Frame(Frame, Stack), Leave(Rc<Prompt>, Stack), Done }
pub struct Handled { pub segments: usize, pub prompt: Rc<Prompt>, pub clause: usize }

pub struct Stack { .. }              // Clone, Default
impl Stack {
    pub fn new() -> Stack;
    pub fn frames(&self) -> usize;   // O(1); this is what bounds recursion now
    pub fn segments(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn push(&self, frame: Frame) -> Stack;
    /// The owned form of `push`, for a caller replacing its own stack.
    pub fn pushed(self, frame: Frame) -> Stack;
    pub fn push_prompt(&self, prompt: Rc<Prompt>) -> Stack;
    pub fn prompt(&self) -> Option<&Rc<Prompt>>;
    pub fn next(&self) -> Next;
    /// The owned form of `next`: the frame is moved out of its link rather
    /// than cloned whenever no captured continuation still holds it.
    pub fn into_next(self) -> Next;
    pub fn find_handler(&self, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>)
        -> Option<Handled>;
    /// Cuts the innermost `segments` segments away. The returned stack is what
    /// the clause runs on; the continuation is what resuming puts back.
    pub fn capture(&self, segments: usize) -> (Continuation, Stack);
    pub fn resume(&self, k: &Continuation) -> Stack;
}

pub struct Continuation { .. }       // Clone
impl Continuation {
    pub fn frames(&self) -> usize;
    pub fn segments(&self) -> usize;
}
```

`capture` copies **one entry per enclosing handler crossed**, never one per
pending frame. That is the property that makes multi-shot affordable, and
required test 15 pins it.

### `ply-eval::machine` — not implemented

```rust
/// A resource limit on the heap the frames live on. Not the bound a runaway
/// recursion hits — that is `limit::DEFAULT_MAX_CALLS`, which both engines
/// share and which a recursion reaches first, since a call costs a frame.
pub const DEFAULT_MAX_FRAMES: usize = 1_000_000;

pub enum Progress { Running, Halted(Value) }

pub struct Machine<'a> { .. }
impl<'a> Machine<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved, check: &'a CheckOutput) -> Machine<'a>;
    pub fn for_program(program: &'a Program, resolved: &'a Resolved) -> Machine<'a>;
    pub fn with_max_frames(self, max: usize) -> Machine<'a>;
    pub fn with_max_calls(self, max: usize) -> Machine<'a>;
    /// The atoms this engine performed at the last entry point.
    pub fn trace(&self) -> &Trace;
    pub fn set_base_world(&mut self, world: World);
    pub fn world(&self) -> &World;
    pub fn test_count(&self) -> usize;
    pub fn test_name(&self, index: usize) -> Option<&'a str>;
    pub fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic>;
    pub fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic>;
    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic>;
    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic>;
    /// One transition. Public so a stepper, a tracer and a fuel budget can each
    /// be written outside the machine.
    pub fn step(&mut self) -> Result<Progress, Diagnostic>;
}
```

The entry points mirror `Interp`'s exactly, which is what makes `Engine` a
drop-in. The transition rules are ADR 0005 §1.3 and are normative — in
particular:

- `W` is threaded through capture and through resumption **unchanged**;
- a tail-resumptive clause runs on the post-capture stack with a `Frame::Resume`
  pushed for it, so `op(x̄) -> e` is exactly `op(x̄) resume k -> k(e)`;
- a clause body runs on the stack *below* its own handler, so a clause that
  performs the operation it handles reaches the next handler out.

### `ply-eval::Engine` — landed

```rust
pub enum Engine { Treewalk, Machine }   // Default = Machine
impl Engine {
    pub fn as_str(self) -> &'static str;
    pub fn parse(s: &str) -> Option<Engine>;
}
```

The default is the authoritative engine and flipping it is a `RUNTIME_VERSION`
bump, which is why it is written here: a cached `Pass` is a claim about what the
authoritative engine did. It is `Machine` as of `RUNTIME_VERSION` 0.4.0.

`Interp` gains `world()` and `set_base_world(World)`; every entry point forks
from the base world rather than starting from an empty one. `Machine` carries the
same two.

Neither type holds an `Engine`. The choice is made once per run, by `ply-test`'s
executor, which constructs an `Interp` or a `Machine` as its `Worker` — the
`Executor` trait already exists for exactly this and already carries no `Send`
bound. A field inside the evaluator would put a branch on the hot path for a
decision that is fixed before the first test starts.

### `ply-syntax` — not implemented

```
hClause := IDENT "." IDENT ("[" IDENT "]")? "(" IDENT,* ")" ("resume" IDENT)? "->" expr ","?
```

```rust
pub struct HandleClause {
    pub effect: QName, pub op: Ident, pub resource: Option<Ident>,
    pub params: Vec<Ident>,
    /// The continuation binder of a general clause.
    pub resume: Option<Ident>,
    pub body: Expr, pub span: Span,
}
```

`resume` is a **contextual** keyword: recognized only between a clause's `)` and
its `->`. `lexer::is_ident("resume")` stays true, no `Kw::Resume` is added, and a
program that binds `resume` as an ordinary name is unaffected. The binder is an
ordinary value binder in the clause body's scope under the ordinary resolution
order.

Bare `-> e` stays tail-resumptive with its current typing rule. Do not make every
clause general: it retypes every handler in every existing program and forces a
capture at every `perform`.

### `ply-core` — not implemented

The handle rule is **unchanged** — a row is a set, so resuming twice does not
change it:

```
footprint(handle body with H) = (row(body) \ handled) ∪ ⋃ row(clause_i) ∪ row(return)
```

A general clause types as:

```
Γ, x̄ : params_i, κ : (ret_i) -> R / ρ_κ  ⊢  body_i : R / ρ_i
```

where `R` is the `handle` expression's result type — the return clause's body
type, or the handled body's type when there is no return clause — and a
tail-resumptive clause keeps `body_i : ret_i`.

`ρ_κ` is solved as follows and an implementer must get both halves right:

- **One `ρ_κ` per `handle`, not per clause.** Every clause's continuation is the
  same residual computation. Allocate it fresh before inferring any clause and
  bind it into every general clause's environment.
- **Solving it drops a self-occurrence in the tail.** `ρ_κ := ρ_h` is
  self-referential because `ρ_h` is built from clause rows that may carry `ρ_κ`
  as their tail. Set union is idempotent, so solve `ρ_κ` to `ρ_h` with `ρ_κ`
  removed from the tail and the fixpoint is reached in one step. This is the
  **only** row variable permitted a self-occurrence; it must be created and
  solved inside `infer_handle`, and general unification's occurs check must not
  change.

`nondet` needs no change: an operation performed once and resumed twice produces
its value once, at the `perform`, and both resumptions receive it.

`ply-core::ty` stays pinned. World-backedness is a claim about the *execution
model* — two tests get two worlds — not about two atoms, so it lives in the
scheduler, below.

### `ply-test` — not implemented

```rust
/// Effects whose atoms name state living in a `World`. Exactly one at v0.
pub const WORLD_BACKED: &[&str] = &["cell"];

pub fn is_world_backed(atom: &EffectAtom) -> bool;
/// Every atom is world-backed. The empty footprint is world-isolated.
pub fn world_isolated(f: &Footprint) -> bool;
/// The atoms that can conflict across tests: `f` minus its world-backed atoms.
pub fn shared_footprint(f: &Footprint) -> Footprint;
```

`group_by_conflict` colours `shared_footprint`s, not raw footprints. A
world-backed atom conflicts with nothing across tests, because each test's cells
are entries in its own forked world and no reference crosses.

Each worker forks the base world per test. `Selection` gains, and `RunReport`
reports:

- `isolation: World | Shared` per test, with the atoms that made it shared;
- `isolated: n of m` in the summary and in `--json`.

Required properties, and they are the milestone's measurable claim rather than a
slogan: every world-isolated test is in group 0 for any number of them; adding
*N* world-isolated tests changes the group count by **zero**; the group count
equals the colouring of the shared tests alone.

### `ply-hash` — not implemented

The `resume` binder **enters normalization**. A clause with a binder is a
different definition from one without. Renaming the binder must change no hash —
it is a local and becomes a de Bruijn level like every other. This is a
`BODY_ENCODING` and a `FRONTEND_VERSION` bump.

Omitting the binder from the hash makes two programs with different semantics
share one cache entry, which is the most expensive defect available in this
system.

### The recursion bound — `ply-eval::limit`

```rust
/// The most nested calls a program may hold at once. Both engines answer to it
/// and phrase the diagnostic identically, because a divergence on a deeply
/// recursive program fails `--engine both` on every corpus that has one.
pub const DEFAULT_MAX_CALLS: usize = 10_000;

/// The deepest a *value* may nest before a structural walk over it — comparing
/// two, diffing two — refuses with the same "recursion limit" message. Equal to
/// `DEFAULT_MAX_CALLS`, so no value the call bound lets a program build is one
/// this bound then refuses to compare; only iteration gets past it.
pub const MAX_VALUE_DEPTH: usize = DEFAULT_MAX_CALLS;
```

A call is not the only thing a program can nest. A value nests too, and every
host recursion the evaluator reaches from source needs an answer that is not an
abort — an abort is not a failure, because it reaches no classifier and loses
every sibling test's result in the same worker. Three answers, and which one
applies is decided by what bounds the depth:

- **depth of data** — `values_equal` and `first_difference` refuse past
  `MAX_VALUE_DEPTH`, because nothing else bounds how deep a value can be built;
- **dropping a value** — `Value`'s `Drop` dismantles iteratively, because a bound
  cannot help: a value has to be dropped whatever its depth;
- **depth of source** — `Interp::eval`, `code::lower`, `Expr::clone`,
  `Infer::infer` and `collect_refs` *grow* the host stack rather than refuse, as
  the parser and the normalizer already do. A bound here would reject on one
  engine a program the front end accepted, which is an `E0503` divergence on
  every corpus with a long operator chain in it.

`Stack::calls()` is the machine's count — the `Frame::Call`s pending, O(1)
through push, pop, capture and splice — and `Interp` counts its own nesting the
same way. A tail call is charged like any other: eliding it left a
tail-recursive runaway unbounded on the machine while the tree-walker diagnosed
it, which is ADR 0005 §7.1.

### The tracer — `ply-eval::trace`

```rust
pub struct Trace { .. }
impl Trace {
    pub fn footprint(&self) -> &Footprint;   // the atoms performed
    pub fn performs(&self) -> u64;           // how many, in total
}
```

Cleared at every entry point, so it describes one test. It records a `perform`,
building its atom exactly as inference builds the declared one; `cell_get` /
`cell_set` are builtins over a `CellId` that carries no resource label, and the
world comparison covers them. `Evaluator::observed_footprint` and
`observed_performs` are how `--engine both` reads it, and a corpus whose
`footprints_compared` is short of `compared` fails the audit.

### `ply-cli` — not implemented

```
--engine <treewalk|machine|both>    default machine
```

On `ply test`, `ply run` and `ply check`. `both` runs each test on each engine and
compares, per test:

- the `Result<(), Diagnostic>` by **full JSON serialization** — code, severity,
  message, every label with its span, every note. Not "both failed";
- the observed footprint from the tracer;
- the final world, as the `(CellId, rendered Value)` sequence from `World::cells`.

A mismatch **fails the run** with a diagnostic naming the test and both outcomes.
Never a warning: a divergence between two evaluators of one language is made
sticky by the cache.

A clause with a `resume` binder is machine-only. The tree-walker must refuse it
with `E0504` (`MACHINE_ONLY_CLAUSE`), naming the clause and saying which engine
runs it — never approximate it as tail-resumptive. Its own code because a
consumer has to tell a refusal to start from a program that ran and failed, and
`ply_eval::is_machine_only` is how the two are separated. Under
`both` such a test runs once, on the machine — `differential::Compared::Refused`
and `Report::machine_only` keep it out of what was compared — and `--explain`
records it as `machine-only`.

Four codes now partition a failing run, and confusing any two of them costs
something specific:

| code | whose fault | what `ply-test` does |
| --- | --- | --- |
| `E0501` / `E0502` | the program's — an assertion, `panic`, division by zero, the recursion limit, a value past `MAX_VALUE_DEPTH` | attributes it: suspects, bisection, culprit |
| `E0503` `ENGINE_DIVERGENCE` | Ply's — two evaluators of one language disagree | `Status::Panicked`, `Skipped::Panicked`, no bisection |
| `E0504` | neither; this engine declined to start | reports it; the other engine answers |
| `E0505` `INTERNAL_ERROR` | Ply's — an evaluator invariant broke | `Status::Panicked`, `Skipped::Panicked`, no bisection |

`Failure::defect` is the observed answer wherever there is one to observe: a run
that watched the evaluator unwind knows something no diagnostic carries. Reading
it off `RUNTIME_ERROR` instead is what made a runaway recursion — a documented
limit, and as bisectable a regression as any assertion — report itself as a
defect in Ply and switch M5 off for the whole class.

`E0503` is the one exception, and only because there is nothing to observe: the
divergence is a comparison the audit *made*, handed back as an ordinary `Err`
rather than as an unwind. Whatever the program means, at most one of the two
answers is it and nothing in the definition graph decides which — so bisecting it
would name whichever definition the disagreement happened to run through.

`ply check --engine treewalk` refuses the same program, and that verdict may not
depend on the front-end cache: gate 1 skips a file whose bytes did not change, so
`ply check` parses any skipped module whose source mentions `resume` before it
scans. A refusal derived from the modules a run happened to parse makes `ply
check` exit 2 cold and 0 warm over untouched source.

`--engine` other than the default implies `--no-cache` in both directions: a
`Pass` in the store is a claim about the authoritative engine, and a
non-authoritative one may neither read nor write one. Flipping the default is a
`RUNTIME_VERSION` bump.

`ply test` also prints `isolated: n of m` in its summary.

### Deleted with the machine

Each of these is a workaround for the native stack, and the explicit stack is
what retires it. They go in the change that deletes the tree-walker, which
follows the flip rather than accompanying it: `--engine both` is what would
catch a bad flip, so it outlives the flip by one change.

| deleted | why |
| --- | --- |
| `grow()`, `stacker::maybe_grow`, the `stacker` dependency | a Ply call costs one `Frame::Call` on the heap |
| `Interp`'s own nesting counter | the bound is `DEFAULT_MAX_CALLS` on `Stack::calls()`, which both engines already share |
| `#[inline(never)]` on the `eval_*` arms | they keep the recursive `eval` frame small |
| `Interp`, `Engine`, `--engine` | one milestone of two evaluators, then one |
| `tests::recursion_to_the_depth_limit_survives_a_one_mebibyte_thread_stack` | it asserts a property that stops existing, not one that fails |

The **semantic** limit stays: a runaway recursion is a diagnostic, not an
out-of-memory kill. Its message keeps the phrase "recursion limit" so ADR 0004's
`AssertionKind::RecursionLimit` still classifies it, and it can now name the
innermost `Call` frames.

### Workspace

`rpds = "1.2.1"` — the persistent `RedBlackTreeMap` a `World` is, parameterized
over the shared-pointer kind so it uses `Rc` and non-atomic refcounts, matching
the existing decision that an `Interp` is confined to one thread. It iterates in
key order, which the byte-identical-artifact rule needs.

The control stack does **not** use `rpds::List`. Its links hold the frame
inline, so pushing one costs a single allocation where `List` — which boxes the
value separately from the node — costs two, and popping an unshared link costs
none where `List` cost two more. A push and a pop are the machine's two most
frequent steps, so that is the difference between the machine's profile being
its own work and being the allocator's.

### Required tests

Numbered as in ADR 0005; `(landed)` marks the ones already passing.

1. Every `ply-eval` unit test passes on both engines, compared by full
   diagnostic equality.
2. `--engine both` over `examples/`, `tests/fixtures/` and the generated corpus
   reports zero divergences.
3. A tree-walker asked to run a `resume` clause refuses and does not evaluate it.
4. Forking a world and writing to the fork leaves the original unchanged.
   *(landed)*
5. Two tests that both retain `cell.write[users]` run in one group and neither
   sees the other's writes.
6. A cell is still readable through a continuation resumed after its `with_cell`
   region returned — a **success**, not an error.
7. A base world seeded once and forked per test gives every test the seeded state
   and no test another's writes.
8. Zero resumptions: the clause sees the writes made before the `perform` and
   none after it.
9. One resumption: `put(5); get()` answers `5`.
10. Two resumptions: the `amb` example evaluates to `30` **and** leaves the trace
    cell at `2`. Both halves are the test.
11. Save-and-restore around each resumption gives each branch the same start.
12. A continuation resumed after its `handle` returned splices onto the
    then-current stack and runs.
13. `op(x) -> e` and `op(x) resume k -> k(e)` agree on result, world and
    footprint on every fixture with a handler.
14. A handler performing the operation it handles reaches the next handler out,
    under both clause forms.
15. Capturing across *n* handlers copies *n* segments regardless of pending frame
    count. *(landed)*
16. A continuation captured inside `map`'s callback and resumed twice produces
    two complete lists.
17. Exceeding `DEFAULT_MAX_FRAMES` is a diagnostic containing "recursion limit"
    whose notes name the innermost `Call` frames.
18. A continuation applied to the wrong number of arguments is `ARITY_MISMATCH`.
19. One footprint for the same program resuming zero, one and two times.
20. `ρ_κ` does not trip the occurs check; a clause calling its own continuation
    infers a closed row.
21. Adding *N* world-isolated tests leaves the group count unchanged, for
    N = 0, 1, 100.
22. `--json` reports `isolation` per test and `isolated: n of m`, agreeing with
    the footprints.
23. A `nondet` operation resumed twice delivers one value to both resumptions.
24. Adding a `resume` binder changes that definition's hash.
25. Renaming the `resume` binder changes no hash.

### Not in M6

- **A source-level `fixture` construct.** M6 lands the mechanism and no syntax.
  This is the milestone's largest gap: "build a fixture once, fork per test in
  microseconds" is demonstrable through the API and not writable in Ply.
- **A world snapshot/restore builtin** — a capability with no type-level account,
  since restoring un-does writes the row still reports. M7.
- **Reclaiming world entries.** Every cheap rule is unsound; correct reclamation
  needs reachability from the live environment graph.
- **One-shot / linearity annotations on continuations.**
- **Effect-typed control operators** beyond `resume`: no `shift`/`reset`, no
  first-class prompts. `handle` is the only delimiter.

---

## Deterministic simulation

`docs/adr/0006-deterministic-simulation.md` has the reasoning; this section is
the contract. **Where it disagrees with any section above, this section wins** —
it was written after them.

### The rule everything else follows from

> A simulated run is a pure function of its definition set and its seed.

Every source of nondeterminism a Ply program can reach is an effect, and
simulation is a handler for it. `simulate` is not a new kind of construct: it is
a `handle` with a fixed clause set whose clauses happen to be written in Rust.

### The prelude effects — landed in `ply-core::prelude`

Declared by the language rather than by a module. Every operation is
singleton-resource; the atoms are `task.write`, `clock.read`, `clock.write`,
`random.write` and `sim.read`.

```ply
nondet effect task {
  write spawn<a | e>(body: () -> a / e) -> Task<a> / e
  write join<a>(t: Task<a>) -> a
  write yield() -> Unit
}

nondet effect clock {
  read  now() -> Int
  write sleep(nanos: Int) -> Unit
}

nondet effect random {
  write next() -> Int
  write below(bound: Int) -> Int
}

effect sim {
  read seed() -> Int
}
```

`task` is `nondet` because concurrency without a specified scheduler *is*
nondeterminism: a `det` test that spawns with no scheduler installed is `E0412`.
`sim` is **not** `nondet` because a seed is an input, and that is the whole
type-level content of the E0412 story below.

`random.next` is a **write**: drawing advances the stream, so two tasks drawing
in the other order get the other values. Declaring it `read` would hide a whole
class of order dependence from the reduction in "Exploration".

An effect declaration whose **program-wide** name equals `task`, `clock`,
`random` or `sim` is `DUPLICATE_DEFINITION`, pointing at the prelude. Only an
anonymous module can produce such a name; `examples/clock.ply`'s `effect clock`
is `clock.clock` and shadows the prelude by the ordinary resolution order.

### `ply-core` — landed

One new field, and nothing in the pinned `ply-core::ty` moves:

```rust
pub struct OpInfo {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub params: Vec<Type>,
    pub ret: Type,
    /// `Some` only for a prelude operation, whose signature is constructed
    /// rather than parsed. `None` for every user-declared operation, which stays
    /// monomorphic and types exactly as it does today.
    pub scheme: Option<Scheme>,
}
```

Perform-site typing: with a scheme, instantiate it, unify the arguments against
its parameters, and union its instantiated row — the `effects` of its `Type::Fn`
— with the performed atom. Without one, today's rule, which is that rule with an
empty row. **One code path.**

`spawn`'s row must carry `e`. Without it a test that spawns a task writing
`db.write[orders]` reports an empty footprint and the cross-test conflict graph
runs it beside a test reading `orders`.

Surface syntax for declaring a polymorphic operation is **not in M7**.

The `simulate` typing rule:

```
Γ ⊢ body : T / ρ_b
handled = { task.write, clock.read, clock.write, random.write }
sim.read ∉ ρ_b                                          (else E0416)
T mentions no Task<_>                                   (else E0413)
────────────────────────────────────────────────────────────────────
Γ ⊢ simulate { body } : T / ( (ρ_b \ handled) ∪ {sim.read} )
```

`cell` is deliberately not in `handled`: a `with_cell` outside a region holding
state the tasks inside share is how tasks share memory.

`Task<a>` is a prelude type constructor. A `Task` in the region's result type is
`E0413`, the same result-type check `with_cell` already uses.

### `ply-syntax` — landed

```
simulate := "simulate" block
```

```rust
pub enum ExprKind {
    // ...
    /// `simulate { body }`. Installs the seeded scheduler over `body`.
    Simulate { body: Box<Expr> },
}
```

`simulate` is a **contextual** keyword, recognized only immediately before a `{`,
exactly as `with_cell` is recognized only immediately before a `[`.
`lexer::is_ident("simulate")` stays true and no `Kw::Simulate` is added.

There is no seed in the syntax. A seed written in source would be part of the
definition's hash, which makes every seed a different definition and every
widening of the search a rewrite of the program.

### `ply-hash` — landed

`Simulate` enters normalization with its own discriminant byte. A `simulate`
region is a different definition from its body, so adding, removing or reordering
one changes the enclosing definition's hash; reformatting it does not. This is a
`BODY_ENCODING` and a `FRONTEND_VERSION` bump, alongside the one `OpInfo::scheme`
forces on the declaration encoding.

### `ply-eval::sim` — landed

The value types every other crate is written against: what a seed *is*, how it
expands into random streams, what a step's accesses are, and how two steps are
decided to commute. `ply-eval::sched` consumes them and `ply-eval::machine`
drives it.

```rust
/// The repro artifact. `root` seeds the streams; `path` is a choice-sequence
/// prefix — at scheduling point `i`, `enabled[path[i]]` is resumed when
/// `i < path.len()`, and the `sched` stream chooses otherwise.
pub struct Seed { pub root: u64, pub path: Vec<u16> }
impl Seed {
    pub fn root(root: u64) -> Seed;
    pub fn at(root: u64, path: Vec<u16>) -> Seed;
    pub fn is_root(&self) -> bool;
    /// `"7"` or `"7:3.0.2"`; the root also accepts `0x`-prefixed hex. Rejects
    /// everything else — a seed that parses loosely replays something other
    /// than what failed.
    pub fn parse(s: &str) -> Option<Seed>;
    /// Canonical bytes for a cache key, length-prefixed. A different job from
    /// the text form: unambiguous rather than readable.
    pub fn to_bytes(&self) -> Vec<u8>;
    /// The interleaving agreeing with this one up to `at` and taking `choice`
    /// there; beyond `at` the stream chooses again. Pads when `at` is past the
    /// recorded path, since a search may reach a point the prefix never named.
    pub fn branch(&self, at: usize, choice: u16) -> Seed;
    pub fn choice(&self, i: usize) -> Option<u16>;
}
impl Display for Seed;                 // "7" or "7:3.0.2"

/// Display: "@3".
pub struct TaskId(pub u32);

pub enum Domain { Sched, Rand }
impl Domain { pub fn as_str(self) -> &'static str; }

/// Counter-mode BLAKE3 rather than a PRNG crate: "the same seed anywhere" is a
/// promise across versions as well as machines, and a generator crate's version
/// is not something this project controls.
pub struct Stream { .. }
impl Stream {
    pub fn new(root: u64, domain: Domain) -> Stream;
    pub fn next_u64(&mut self) -> u64;
    /// Rejection sampling, specified exactly: with `limit = (u64::MAX / n) * n`,
    /// draw until `x < limit`, answer `x % n`. `n == 0` is `None`, which the
    /// `random.below` builtin turns into `RUNTIME_ERROR`.
    pub fn below(&mut self, n: u64) -> Option<u64>;
    pub fn drawn(&self) -> u64;
    /// Pure, so a replay can ask for draw `counter` without serving the ones
    /// before it.
    pub fn draw(root: u64, domain: Domain, counter: u64) -> u64;
}

pub enum SimMode { Once, Random, Dpor }   // Default = Dpor
impl SimMode {
    pub fn as_str(self) -> &'static str;
    pub fn parse(s: &str) -> Option<SimMode>;
    /// Whether a root's exploration decomposes into independent per-seed claims.
    /// Only `Random`'s does.
    pub fn caches_per_seed(self) -> bool;
}

pub const DEFAULT_BUDGET: u32 = 256;        // interleavings per root, dpor
pub const DEFAULT_STEPS: u32 = 100_000;     // scheduling steps per interleaving
pub const DEFAULT_RANDOM_ROOTS: u32 = 64;

pub struct Plan {
    pub mode: SimMode,
    /// Ascending and deduplicated by `normalized`.
    pub roots: Vec<u64>,
    pub budget: u32,
    pub steps: u32,
    /// The fixed path under `Once`, so `--seed 7:3.0.2` names one interleaving
    /// rather than one root. Empty in every other mode.
    pub path: Vec<u16>,
}
impl Plan {
    pub fn once(seed: Seed) -> Plan;
    pub fn random(roots: u32) -> Plan;
    pub fn normalized(self) -> Plan;
    pub fn seeds(&self) -> Vec<Seed>;
    /// Covers every field, length-prefixed. Two plans that search the same thing
    /// have the same digest, or the cache splits on the order a caller happened
    /// to write its flags in.
    pub fn digest(&self) -> [u8; 32];
}
impl Default for Plan;   // Dpor, roots [0], DEFAULT_BUDGET, DEFAULT_STEPS

/// One access a step made. Finer than a `Footprint` in exactly one place: a cell
/// is a *location*, so it is keyed by `CellId` rather than by the `[r]` label
/// several cells may share.
pub enum Access {
    Atom(EffectAtom),
    Cell { id: CellId, mode: Mode },
    /// A `with_cell` took the next id from the world's own counter. Allocation
    /// has no location to name, so it is its own kind of access.
    Alloc,
}
impl Access {
    /// Two `Atom`s by `EffectAtom::conflicts_with`; two `Cell`s iff the same
    /// `CellId` and at least one is a `Write`; two `Alloc`s always, since they
    /// take ids from one counter; anything else never.
    pub fn conflicts_with(&self, other: &Access) -> bool;
    pub fn is_write(&self) -> bool;
}
impl Display for Access;   // "db.write[users]", "cell.write[#7]" or "cell.alloc"

pub struct StepFootprint { .. }
impl StepFootprint {
    pub fn new() -> StepFootprint;
    pub fn from_accesses(accesses: impl IntoIterator<Item = Access>) -> StepFootprint;
    pub fn insert(&mut self, access: Access);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn accesses(&self) -> impl Iterator<Item = &Access>;
    /// **The dependence relation.** Two steps that do not conflict commute.
    pub fn conflicts_with(&self, other: &StepFootprint) -> bool;
    /// What the two contend over, as a diagnostic renders it. Empty exactly when
    /// they commute.
    pub fn contention(&self, other: &StepFootprint) -> Vec<&Access>;
}

/// One end of a race, as the failure artifact prints it. Not a *step* — that is
/// the unit the scheduler runs; this is where one of them was standing.
pub struct RaceSite {
    pub task: TaskId,
    /// `None` when the failure was not traced. Never guessed.
    pub definition: Option<Symbol>,
    pub access: String,
    pub span: Span,
}
/// The two steps whose reordering flipped a passing interleaving to a failing
/// one. `Some` only when the search observed the flip.
pub struct Race { pub left: RaceSite, pub right: RaceSite, pub at: u32 }

/// `bounded` means the naive search spent its own budget, so the count is a
/// lower bound and renders `>= n`.
pub struct Naive { pub explored: u32, pub bounded: bool }
impl Display for Naive;

pub struct Exploration {
    pub explored: u32,
    /// The frontier emptied within the budget: **every** interleaving ran.
    pub exhaustive: bool,
    /// The budget was spent.
    pub exhausted: bool,
    /// `--measure-reduction` only.
    pub naive: Option<Naive>,
    pub steps: u64,
    /// Nanoseconds of virtual time the last interleaving consumed.
    pub virtual_time: i64,
    pub failure: Option<Seed>,
    pub race: Option<Race>,
}
impl Exploration {
    /// `None` unless the naive count was measured. Never computed from an
    /// assumption.
    pub fn reduction(&self) -> Option<f64>;
    /// `failure.is_none() && !exhausted`. An exhausted search proved nothing
    /// about the interleavings it did not reach, so its green verdict may not be
    /// written to the result cache.
    pub fn is_cacheable(&self) -> bool;
}
```

Requirements the types do not carry:

- **`ply-eval::sim` may name no hash-based collection.** Run queues are `Vec` in
  insertion order, sets are `BTreeSet`. A rule about how a type is *used* is a
  rule nobody enforces; a rule about which types may be *named* is greppable, and
  a landed test greps this module for `HashMap`, `HashSet`, `FxHashMap`,
  `FxHashSet`, `SystemTime`, `Instant`, `thread::`, `rayon`, `as_ptr` and
  `strong_count`.
- No decision may read a pointer, an address, a refcount or an allocation order.
- The `sched` and `rand` streams have **separate counters**. Sharing one makes
  adding a `random.next()` call shift the interleaving, so a change to the data
  becomes a change to the schedule and a bisection over it names the wrong
  definition. A landed test pins it.
- Virtual time is nanoseconds since the region was entered, starting at `0`.

### `ply-eval::machine` and `ply-eval::region` — landed

```rust
impl<'a> Machine<'a> {
    /// The seed the next entry point's `simulate` region runs at, and the
    /// scheduling steps one interleaving may spend.
    pub fn set_seed(&mut self, seed: Seed, steps: u32);
    /// What the last entry point's `simulate` regions did — **every** region it
    /// entered, over one choice sequence. `None` when it reached none, which is
    /// how a test with no region pays nothing.
    pub fn simulated(&self) -> Option<&region::Record>;
}

impl region::Record {
    /// The interleaving, given how the entry point that produced it ended. The
    /// verdict is the *run's*: a region that completed inside a test whose later
    /// assertion failed is a failing interleaving.
    pub fn interleaving(&self, outcome: &Result<(), Diagnostic>) -> Interleaving;
}
```

**One trail per entry point, not one per region.** A test may enter several
`simulate` regions in sequence — only *nesting* is `E0416`, and an ordinary call
reaches one region twice with no syntax pointing at it. Scheduling point *i* is
the *i*th of the **run**: one `Seed::path`, one `sched` stream, one step record,
one `rand` counter, shared by every region the entry point enters, and each step
tagged with the region that took it so the search never pairs two that ran in
sequence. A record covering one of several regions is a search input describing
that region and an `exhaustive: true` asserted about all of them, and a per-region
choice counter makes one seed mean a different thing in each region — which
surfaces as `E0415` on a program that is merely unusual.

The machine runs **one** interleaving per entry point — the one its seed names —
and the search over the others is `ply-test`'s, which re-runs the whole test per
interleaving from a fresh fork of the base world. That split is what §2.4
requires: exploration is a test-time activity, and `ply run` explores exactly one
interleaving. `Executor::execute` is therefore **unchanged**, and so are
grouping, panic containment and the per-test cache rules.

`ply_test::sim::seed_run` and `ply_test::sim::interleaving_of` are the whole of
the seam, which is why `Evaluator` gains nothing: the tree-walker refuses a
region outright and has no interleaving to report.

The scheduler is a **native prompt**: a delimiter on the M6 stack whose clauses
are Rust. `Segment` gains a native form and `Stack::find_handler` consults both;
capture, splice, deep handlers and the threaded world are all unchanged. A task
is `(TaskId, Continuation, TaskState)`, and resuming one is the transition
ADR 0005 §1.3 already specifies for applying a continuation.

Six rules an implementer must get exactly right:

- **A step** runs from the scheduler's resumption of a task up to and including
  that task's next scheduler-visible perform. Its access set **excludes** the
  terminating `task.*` / `clock.*` atom — including it makes every pair of steps
  dependent and the reduction exactly 1× — and **includes** `random.write`, whose
  value the program observes rather than the scheduler.
- **The machine must record cell accesses,** which ADR 0005's tracer does not:
  it records a `perform`, and `cell_get` / `cell_set` are builtins over a
  `CellId` that carries no resource label, so the tracer skips them. Under
  simulation a cell is the *main* way two tasks share state, so a step's
  `StepFootprint` carries `Access::Cell { id, mode }` per `cell_get` and
  `cell_set`. A build that omits them explores one interleaving of every
  cell-backed race in the corpus and reports a large reduction for it.
- **…and cell allocations,** which are neither. `with_cell` takes the next id
  from the world's own counter, so two tasks that each open a private cell reach
  two *different worlds* depending on their order — and §6.1's soundness
  condition says two independent steps reach the same world. A `with_cell` inside
  a region contributes `Access::Alloc`.
- **Control that abandons a region is a diagnostic, never a truncated trace.** A
  handler outside the region may capture across its delimiter; if it never
  resumes, the region's tasks are destroyed unfinished and every step after that
  point is missing from the recording, so DPOR's completeness precondition —
  every explored execution runs all processes to completion — is violated and the
  search reports `exhaustive` over schedules it cut short. A run that ends with a
  region still live is `E0413`. A continuation that re-enters a region which has
  already delivered its value is `E0413` too: its scheduler is gone. A
  resumption that *is* legal moves the region's anchor — the stack it delivers
  its value onto is the one the splice put it over, not the one `simulate` was
  entered on, or everything the resuming clause still had pending is lost.
- **Enabledness, not dependence, carries synchronization.** A task blocked on a
  `join` or a timer is not in the enabled set, so no schedule that runs it early
  is ever generated. Encoding a join as a conflict produces a search that
  generates impossible schedules and then prunes them.
- **The value and world a region delivers are those of the interleaving its seed
  names.** Every other interleaving is a search and its world is discarded, so
  `--sim-budget` changes the thoroughness of a test and never the meaning of a
  program.

`ply run` explores exactly one interleaving — the one its seed names. Exploration
is a test-time activity.

`simulate` is **machine-only**: the tree-walker refuses it with `E0504` exactly as
it refuses a `resume` clause, and `machine_only_clauses` learns to scan for it.

### Exploration — landed in `ply-eval::explore`

Dynamic partial-order reduction in the backtrack-set formulation, with
`StepFootprint::conflicts_with` substituted for the alias analysis the literature
has to approximate.

```
explore(prefix):
    run the test at Seed { root, path: prefix }
    record steps s₁..sₙ with access sets A₁..Aₙ and enabled sets E₁..Eₙ
    if the run failed: report it, with this seed
    for i = n down to 1:
        for each j < i with  Aⱼ ⋈ Aᵢ
                       and  task(sⱼ) ≠ task(sᵢ)
                       and  no k in (j, i) has task(s_k) = task(sᵢ)
                       and  task(sᵢ) ∈ Eⱼ:
            backtrack[j] ∪= { task(sᵢ) }
    for each t in backtrack[j] not yet explored at j:
        explore(prefix[0..j] ++ [index of t in Eⱼ])
```

- Sleep sets are **not in M7**. Backtrack sets alone are sound.
- **Replay is self-checking.** Re-running a prefix must reproduce the recorded
  enabled set at every choice point it names; a mismatch is `E0415`.
- **`ply_test::shared_footprint` must not be used here.** It drops `cell` atoms
  because two *tests* hold two worlds. Two *tasks* hold one world, so dropping
  them prunes away every shared-memory race in the corpus while reporting a
  larger reduction for having done it. Required test 22 exists for this mistake
  and nothing else.
- **`--measure-reduction`** runs the same search a second time with the
  dependence relation forced to `true`, which degenerates DPOR into exhaustive
  enumeration of every schedule respecting per-task order and enabledness. Same
  code, one flag, no second implementation to disagree with the first. A spent
  naive budget is reported as `naive >= n`, never as an exact count nobody
  observed.

### The clock — landed in `ply-eval::sim`

> Virtual time advances at exactly one moment: when no task is enabled and at
> least one is blocked on a timer. It jumps to the earliest deadline among them,
> and every task whose deadline equals that time becomes enabled at once.

`clock.now()` observes virtual time and does not move it. `clock.sleep(d)` blocks
until `now + d`; `d <= 0` is a yield. There is no `clock` stream and no jitter: a
timeout that can fire while work is still pending is a timeout that can fire
spuriously, and the exactness is worth more than the extra schedules the
scheduler's own choices already cover.

When no task is enabled and no timer can fire, the region is `E0414`.

Virtual time does **not** advance for computation. A simulated test cannot detect
that an implementation got slower and must not be read as a performance test.

### `ply-test::schedule` and `ply-test::key` — landed

```rust
/// Effects the language simulates. `simulate` discharges exactly these and
/// nothing else.
pub const SIMULATED: &[&str] = &["task", "clock", "random"];
/// The seed effect. Deliberately not `nondet`: a seed is an input.
pub const SIM_EFFECT: &str = "sim";
/// Effects naming an input no test can write. Exactly one at v0: `sim`.
pub const AMBIENT: &[&str] = &["sim"];

pub fn is_ambient(atom: &EffectAtom) -> bool;
/// Neither world-backed nor ambient: the atoms that can contend across tests.
pub fn contends(atom: &EffectAtom) -> bool;
/// This test's outcome is a function of its definitions **and** a seed.
pub fn is_seeded(f: &Footprint) -> bool;

/// The cache key of a seeded test: its definitions and the whole plan searched.
/// A `DefHash`, so `Store` needs no new shape; domain-tagged with
/// `b"ply.sim.key.1"`, so it cannot collide with a definition's own hash.
pub fn sim_key(test_hash: DefHash, plan: &Plan) -> DefHash;
/// The per-root key `random` mode additionally writes, under
/// `b"ply.sim.seed.1"`.
pub fn seed_key(test_hash: DefHash, seed: &Seed) -> DefHash;
/// Whether a plan's per-root results may be cached individually.
pub fn writes_seed_keys(plan: &Plan) -> bool;
/// The key a test's result belongs under. `seeded` is `is_seeded` over the
/// test's footprint.
pub fn result_key(test_hash: DefHash, seeded: bool, plan: &Plan) -> DefHash;
```

`world_isolated` widened from "every atom is world-backed" to "no atom contends",
because `sim.read` must not drop every simulated test out of the `isolated: n of
m` number for no reason. `shared_footprint` drops ambient atoms with the
world-backed ones. Both are landed, with tests.

### `ply-test` — landed

```rust
pub fn select(check: &CheckOutput, hashes: &HashOutput, store: &Store, plan: &Plan)
    -> Selection;
```

A breaking change to `select`, deliberately rather than a `select_with_plan`
beside it: a caller that kept the old signature while running a non-default plan
would read and write the wrong cache entry silently, which is the one failure
mode this section exists to prevent.

**Cache rules, and getting these wrong is the milestone's most expensive defect:**

| test | key | |
| --- | --- | --- |
| no `sim.read` in its footprint | `test_hash` | unchanged |
| `sim.read`, any mode | `sim_key(test_hash, plan)` | **never** the bare `test_hash` |
| `sim.read`, `random` mode | additionally `seed_key` per root | a widened root set runs only the new roots |
| `sim.read`, `dpor` mode | plan key only | a root's exploration does not decompose |

- **An exhausted search writes nothing.** A green run that spent its budget is
  reported green and re-runs next time, because it proved nothing about the
  interleavings it did not reach. This is the first green `det` test in the
  language that is not cacheable, and it is correct that it is not.
- A failing test is not cached, unchanged.

`Failure` gains `seed: Option<Seed>` and `race: Option<Race>`, both `None` when
nothing observed them. **M5's hybrids run at the failing seed**: `BodyHybrid` uses
`Plan::once(failing_seed)`. A hybrid that explores its own interleavings answers a
different question, and a bisection over it names whichever definition the other
interleaving ran through.

`AssertionKind` gains `Deadlock`, so the classifier still partitions the space.

### `ply-cli` — landed

```
--seed <SEED>              a `Seed`: "7" or "7:3.0.2". Implies `--sim once`.
--sim <once|random|dpor>   default dpor
--sim-roots <N>            default 1 under dpor, 64 under random
--sim-budget <N>           interleavings per root, default 256
--sim-steps <N>            scheduling steps per interleaving, default 100_000
--measure-reduction        also run the unpruned search and report the naive count
```

On `ply test` and `ply run`. Under `ply run` only `--seed` has an effect.

Summary and `--explain`:

```
   simulated: 3 of 47 · 61 interleavings · 3 exhaustive

   ✓ transfers are atomic under any interleaving
       12 interleavings · exhaustive · naive 720 · 60× reduction        3.1ms
   ✗ balance never goes negative                                        8.2ms
       seed: 0:1.0.3 · 47 interleavings
       race: @1  apply_debit   db.write[accounts]   src/ledger.ply:31:5
             @2  apply_debit   db.write[accounts]   src/ledger.ply:31:5
       replay: ply test --seed 0:1.0.3 --filter "balance never goes negative"
       culprit: apply_debit
```

`--json` carries `"schema_version": 3`: `failures[].seed`, `failures[].race`, and
per test a `simulation` object of `explored`, `exhaustive`, `exhausted`, `naive`,
`steps` and `virtual_time_ns`. Absent on a test that reached no region — never
zero, which a consumer cannot tell from "no region".

### New diagnostic codes — landed

Added to `ply_span::codes`; existing numbers are unchanged, and the registry pin
test covers all four.

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0413 | `TASK_ESCAPES_SCOPE` | a `Task<_>` in a region's result type, or a `join` of a task whose region ended | the program's |
| E0414 | `DEADLOCK` | nothing is enabled and no timer can fire, **or** the step budget was spent | the program's |
| E0415 | `SIMULATION_DIVERGENCE` | a replay did not reproduce the recorded enabled sets | **Ply's** |
| E0416 | `NESTED_SIMULATION` | `sim.read` is in a `simulate` body's row | the program's |

`E0414` covers deadlock and livelock under one code with two messages: from the
program's side both are "this stopped making progress" and the fix is in the same
place. The message names the blocked tasks and what each waits on.

E0413, E0414 and E0416 join `E0501`/`E0502`'s row — the program is at fault,
`Failure::defect` is `false`, and they are attributed and bisected like any other
failure. E0415 joins `E0503`'s — Ply's fault, `Status::Panicked`,
`Skipped::Panicked`, no bisection, for the same reason: the run knows the two
answers disagree and nothing in the definition graph decides which the program
meant.

### Versions

`RUNTIME_VERSION` bumps to `0.5.0`; `FRONTEND_VERSION` and `BODY_ENCODING` bump
for `Simulate` in the AST and `OpInfo::scheme` in the declaration encoding.
`ply-eval` gains `blake3`, already a workspace dependency at 1.8.5.

### Required tests

The ADR's numbering; forty of them, and these are the ones whose absence would
let the milestone ship broken rather than merely incomplete.

1. Two tasks incrementing one cell-backed counter: some interleaving loses an
   update, and the search finds it.
2. A `det` test spawning with no scheduler is `E0412` naming `task.write`.
3. A user-written sequential `task` handler discharges `task`; the test is `det`
   and caches under its bare `test_hash`.
4. A `Task` in a region's result type is `E0413`.
5. Joining a task through a continuation resumed after its region ended is
   `E0413`, not a wrong answer.
6. A task still runnable when the body returns is run to completion first.
7. A join cycle is `E0414` naming both tasks and what each waits on.
8. One seed twice in one process: identical outcome, step sequence and final
   world.
9. `--jobs 1` and `--jobs 16` produce byte-identical `--json` at one seed.
10. `Seed::parse` round-trips `7` and `7:3.0.2` and rejects everything else.
11. Adding a `random.next()` call changes no scheduling choice that precedes it.
12. `random.below` is unbiased by the specified rule; `n <= 0` is
    `RUNTIME_ERROR`.
13. `ply-eval::sim` names no hash-based collection.
14. `clock.sleep(30_000_000_000)` costs no measurable wall clock and advances
    virtual time by exactly that much.
15. Virtual time does not advance while any task is enabled.
16. Two tasks sleeping to one deadline both wake at it, and their order is
    explored.
17. A timeout does not fire while another task can still run.
18. Two tasks touching disjoint resources produce **one** interleaving, and the
    naive count for the same program is greater than one.
19. Two tasks writing one resource produce both orders.
20. Read/read produces one interleaving; read/write produces two.
21. Two tasks writing two different `CellId`s sharing a `[r]` label do not
    conflict.
22. A region built on `shared_footprint` fails: `cell` accesses must be in the
    relation.
23. `--measure-reduction` reports an exact naive count on a fixture small enough
    to enumerate by hand, and a `>=` bound when its budget is spent.
24. An emptied frontier is `exhaustive: true`; a spent budget is
    `exhaustive: false, exhausted: true`.
25. A replay whose enabled set does not match is `E0415`, classified as a defect
    rather than bisected.
26. `simulate` inside `simulate`, lexically and through a call, is both `E0416`.
27. A user's own `nondet effect` inside a region is still `E0412`.
28. `clock.now()` outside any region in a `det` test is still `E0412`.
29. A handler answering `sim.seed()` closes `sim.read` out of the row and the
    test caches under its bare hash.
30. Spawning a task that performs `db.write[orders]` puts that atom in the
    spawner's row, the test's footprint and the cross-test conflict graph.
31. Adding N simulated, otherwise-isolated tests changes the group count by zero.
32. A seeded test is never written under its bare `test_hash`.
33. Changing `--sim-budget` changes the key and re-runs; changing nothing re-runs
    nothing.
34. Under `random`, widening 64 roots to 128 runs 64 tests, not 128.
35. Under `dpor`, an exhausted search writes no `Pass`.
36. A bisection over a simulated failure runs every hybrid at the failing seed.
37. `simulate` under `--engine treewalk` is `E0504` and does not evaluate.
38. Under `--engine both` a test containing `simulate` runs once, on the machine,
    and `--explain` records it as `machine-only`.
39. Adding, removing or reordering a `simulate` region changes the enclosing
    definition's hash; reformatting it does not.
40. `--sim-budget 1` and `--sim-budget 256` deliver the same value and final
    world for a passing program.

Plus one `tests/fixtures/` entry per new code, as every milestone owes.

### Not in M7

- **Real threads.** The simulated handler runs on one thread. `rayon` stays at
  the test-runner level, scheduling whole tests.
- **A real network.** ROADMAP's M7 line says "network"; M7 delivers the mechanism
  and no network effect. Partitions, reordering, duplication and partial writes
  are each a modelling decision. **This is the largest gap in the milestone.**
- **Finding races in Rust code.** The races found are between Ply tasks over Ply
  resources.
- **Cancellation, a timeout primitive, channels, mutexes.** Cells plus
  `spawn`/`join`/`yield` is the primitive set; the rest is a library, and a
  library written in Ply is one whose handlers the effect system can see.
- **Sleep sets** or any DPOR refinement beyond backtrack sets.
- **Schedule minimization.** The race pair is the actionable half and it is exact.
- **Surface syntax for polymorphic operation signatures.**
- **Simulating a user-declared `nondet` effect.** There is no way to ask the
  language to simulate `http`. A user simulates their own effect by writing a
  handler, which is what handlers are for.

---

## Specs

`docs/adr/0007-specs.md` has the reasoning; this section is the contract.
**Where it disagrees with any section above, this section wins** — it was written
after them.

### The rule everything else follows from

> A spec is a claim *about* a definition, not part of it. An obligation is
> discharged at the strongest tier the system can **demonstrate**, and the tier is
> derived from the evidence rather than asserted alongside it.

And the rule that overrides every convenience below:

> **A tier label is a truth claim.** Reporting `proved` for something that was
> sampled is the worst defect this project can ship. When in doubt, report the
> weaker tier.

### `ply-syntax` — landed

```
fnDef      := "pub"? "fn" IDENT generics? "(" params ")" ("->" type)? ("/" row)?
              specClause* ("=" expr | block)
specClause := ("requires" | "ensures") expr
lawDef     := "law" STRING ("forall" "(" binder,* ")")? ("where" expr)? block
binder     := IDENT ":" type
item       := "pub"? (fnDef | typeDef | effectDef) | testDef | lawDef
```

```rust
pub enum Item {
    Fn(Box<FnDef>), Type(Box<TypeDef>), Effect(Box<EffectDef>),
    Test(Box<TestDef>),
    /// `law "label" forall (x: T) where g { body }`. Labelled like a `test`,
    /// never `pub`, and nothing can reference it.
    Law(Box<LawDef>),
}

pub enum SpecKind { Requires, Ensures }

pub struct SpecClause { pub kind: SpecKind, pub expr: Expr, pub span: Span }

/// A `forall` binder. The type is mandatory, so this is not a `Param`.
pub struct Binder { pub name: Ident, pub ty: TypeExpr, pub span: Span }

pub struct LawDef {
    pub name: String,
    pub name_span: Span,
    pub binders: Vec<Binder>,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

pub struct FnDef { /* … */ pub spec: Vec<SpecClause>, /* … */ }
```

`requires`, `ensures`, `law`, `forall`, `where` and `result` are all **contextual**
— `lexer::is_ident` stays true for every one, no `Kw` variant is added, and a
program that already uses any of them as a name is unaffected. `law` is
unambiguous at item position because nothing else starts an item with a bare
identifier. A clause expression and a `where` guard are parsed with the parser's
existing `no_brace` flag set, exactly as an `if` condition is, so
`ensures p(x) { .. }` is a clause plus a block body and never a record literal.

`Item::Law` returns `None` from `Item::name()` and `Visibility::Private` from
`Item::visibility()`, exactly as `Item::Test` does. Two laws with one label in one
module are `DUPLICATE_DEFINITION`.

### `ply-core` — landed

```rust
pub struct SpecInfo {
    pub kind: SpecKind,
    /// Position among the owner's clauses, in source order. Part of the key.
    pub index: usize,
    /// Always empty. Carried rather than assumed so an audit asserts on a value.
    pub footprint: Footprint,
    pub span: Span,
}

pub struct LawBinder { pub name: Symbol, pub ty: Type, pub span: Span }

pub struct LawInfo {
    pub name: String,
    pub module: ModuleName,
    /// `<module>.<label>`, unique program-wide, as `TestInfo::key` is.
    pub key: Symbol,
    pub index: usize,
    pub binders: Vec<LawBinder>,
    pub has_guard: bool,
    /// `{}`, or `{sim.read}` for a concurrency law. Nothing else type-checks.
    pub footprint: Footprint,
    pub span: Span,
}

pub struct DefInfo { /* … */ pub spec: Vec<SpecInfo>, /* … */ }
pub struct CheckOutput { /* … */ pub laws: Vec<LawInfo>, /* … */ }
```

The purity rule, enforced here:

```
Γ, params ⊢ e : Bool / ρ                     ρ.is_pure()   (else E0417)   requires
Γ, params, result : T ⊢ e : Bool / ρ         ρ.is_pure()   (else E0417)   ensures
Γ, binders ⊢ g : Bool / ρ_g                  ρ_g.is_pure() (else E0417)   where
Γ, binders ⊢ b : Bool / ρ_b            ρ_b ⊆ {sim.read}    (else E0417)   law body
```

`Row::is_pure()` is `atoms.is_empty() && tail.is_none()`; the tail matters,
because a row variable is not pure for every instantiation. `{sim.read}` in a law
body is the one exception and makes it a **concurrency law**; a `simulate` in a
`requires`, an `ensures` or a `where` is `E0417`.

`result` is bound in an `ensures` clause **beside** the parameters, not inside
them: `result` in a `requires` is `UNKNOWN_NAME` with a note, and a parameter
named `result` on a definition carrying an `ensures` is `DUPLICATE_DEFINITION`
pointing at both. A definition with no `ensures` may still name a parameter
`result`, which `examples/timeout.ply` relies on.

A `forall` binder's type must be quantifiable: no `Cell<_>` or `Task<_>`, no
function type with a non-empty row, no effect-row variable. Otherwise `E0418`.
A type variable **is** allowed: the prover treats it as an uninterpreted sort and
the property tier monomorphises it to `Int` and says so.

**Attaching a spec changes no footprint.** `DefInfo::footprint`, `E0412`,
`Isolation` and the cross-test conflict graph are all unaffected. Required test.

### `ply-hash` — landed shape, implementation outstanding

Specs and laws are **erased by normalization**, so:

> Adding, editing or deleting a `requires`, an `ensures` or a `law` changes zero
> definition hashes and selects zero tests.

That is a headline invariant of the same shape as "renaming a function selects
zero tests" and it is a required test. A law is nevertheless hashed like a test —
its own item discriminant, its binder types, guard and body normalized together —
because it is an item with a body.

```rust
pub struct HashOutput {
    // …
    /// Parallel to `CheckOutput::laws`.
    pub laws: Vec<DefHash>,
    /// Definition program-wide name -> one hash per clause, in source order.
    pub specs: IndexMap<Symbol, Vec<DefHash>>,
    /// The same clauses and laws, identified as *sentences*: references by
    /// name, and no owner hash. Read only by the review baseline.
    pub spec_texts: IndexMap<Symbol, Vec<DefHash>>,
    pub law_texts: Vec<DefHash>,
}

spec_hash(owner, kind, index, clause) =
    blake3( b"ply.spec.1" ‖ owner_def_hash ‖ kind_u8 ‖ index_le_u32
          ‖ normalize(clause) )

spec_text_hash(kind, index, clause) =
    blake3( b"ply.spec.text.1" ‖ kind_u8 ‖ index_le_u32
          ‖ normalize_by_name(clause) )
```

`owner_def_hash` is first and is not optional: a key that omitted it would leave
a discharged `ensures` discharged after its definition was rewritten, which is a
cached proof of something no longer true. That is the permissive-direction
failure and it is the one that must not ship.

`kind_u8` is 1 for `requires`, 2 for `ensures`. `normalize(clause)` is the
ordinary body encoding: the owner's parameters and `result` become de Bruijn
levels, a free reference contributes the referent's hash, names and spans are
erased. `BODY_ENCODING` and `FRONTEND_VERSION` bump for the law discriminant.

**`spec_texts` and `law_texts` answer a different question and must not be
confused with the keys above.** An obligation key covers `owner_def_hash`, so it
moves whenever the implementation moves — which is exactly what re-opens a proof
about rewritten code. That makes it useless for review, where *did the claim
change?* has to be answerable independently of *did the implementation change?*.
A sentence hash therefore drops the owner and writes each reference as the
referent's **name** rather than its hash, so it moves when the sentence is
rewritten and stays put when the definitions it mentions are re-implemented.
Nothing is selected, cached or discharged on a sentence hash; if it were, a
definition's identity would depend on its dependencies' names and content
addressing would be gone. Without the distinction, §9.2's *implementation
changed · spec unchanged* row — the cheapest review in the system — is
unreachable, because every body edit also reads as a spec edit.

### `ply-store` — landed shape, implementation outstanding

```rust
/// Bumping this re-attempts every obligation and re-runs no test.
pub const PROVER_VERSION: &str = "0.1.0";

impl Store {
    pub fn obligation(&self, key: DefHash) -> Option<Evidence>;
    pub fn put_obligation(&mut self, key: DefHash, evidence: &Evidence);
    pub fn review_record(&self, def: &Symbol) -> Option<&ReviewRecord>;
    pub fn put_review_record(&mut self, def: Symbol, record: ReviewRecord);
}

/// `specs` holds *sentence* hashes — `HashOutput::spec_texts` for this
/// definition's own clauses, plus `law_texts` for every law naming it directly.
pub struct ReviewRecord { pub def_hash: DefHash, pub specs: Vec<DefHash> }
```

The store persists an **`Evidence`**, not a `Discharge`, and that is deliberate:
`Evidence` has no variant for a refutation, a vacuity or a gap, so a cache that
held one would not type-check. `Discharge` is not `Serialize` for the same
reason — `ply-cli` projects it into the JSON artifact, exactly as it already
projects `ply_test::Failure`.

Two new files under `.ply-cache/`, `obligations.json` and `reviews.json`, both
**read lazily on the first question** and neither at `Store::open` — ADR 0003's
addendum measured what folding a per-item payload into `results.json` costs, and
the answer was three times the open budget. The namespace is keyed on
`(PROVER_VERSION, key)`, independent of `RUNTIME_VERSION`.

`SourceFingerprint` gains `specs`, so a file gate 1 skipped still contributes its
obligations. **Spec clauses are never skipped by gate 2**: a spec is erased from
the definition's hash, so a spec edit does not move it, so a definition restored
from `KnownDef` still has its clauses typed against the restored `Scheme`, every
run in which its file was parsed.

### `ply-prove` — landed

A new crate. Depends on `ply-span`, `ply-syntax`, `ply-core`, `ply-hash` and
`ply-eval`; nothing depends on it but `ply-cli`. It deliberately does **not**
depend on `ply-test`: obligations are not tests, and every obligation is pure, so
there is no conflict graph, no colouring, and every obligation is in group 0 for
any number of them.

```rust
pub enum Tier { Example, Property, Proved }          // Ord: Example < Proved

pub const MIN_PROPERTY_CASES: u32 = 25;
pub const UNFOLD_DEPTH: u32 = 3;
pub const ENUMERATION_BOUND: u64 = 4096;
pub const GEN_DEPTH: u32 = 4;

pub enum Rule {
    GroundEvaluation,
    ExhaustiveEnumeration { domain: Symbol, points: u64 },
    LinearArithmetic,
    Propositional,
    CaseSplit { ty: Symbol, arms: u32 },
    Congruence,
    Injectivity,
    Unfold { def: Symbol, depth: u32 },
    /// The one certificate rule that comes from execution.
    ExhaustiveInterleaving { interleavings: u32 },
}

pub struct Certificate {
    pub rules: Vec<Rule>,
    pub steps: u32,
    /// A proof over an empty domain is not a proof. Always true on a `Held`.
    pub guard_satisfiable: bool,
    /// Type variables the proof left uninterpreted, so the claim is polymorphic.
    pub sorts: Vec<Symbol>,
}

pub struct CaseReport {
    pub generated: u32,
    pub kept: u32,
    pub rejected: u32,
    pub roots: Vec<u64>,
    /// Type variables monomorphised for generation, e.g. `a := Int`.
    pub instantiations: Vec<(Symbol, Type)>,
}

pub enum Evidence { Proof(Certificate), Cases(CaseReport) }

pub enum Discharge {
    Held(Evidence),
    Refuted(Counterexample),
    Vacuous(Vacuity),
    Unattempted(Gap),
}
```

**There is no `tier` field anywhere.** `Evidence::tier()` computes it:

```rust
Evidence::Proof(_)                              => Tier::Proved
Evidence::Cases(c) if c.kept >= MIN_PROPERTY_CASES => Tier::Property
Evidence::Cases(_)                              => Tier::Example
```

so a component that wants to report `proved` must hand over a `Certificate`,
which only the prover can construct and which names the rules it used. `Rule` is a
closed enum, so a prover that grows a rule nobody sanctioned stops compiling
before it fails an audit. This is the structural form of the overriding rule; it
is not a convention and must not be softened into one.

```rust
pub enum Frame {
    /// The footprint is empty: the result is a function of the arguments.
    Pure,
    /// Every resource outside this set is unchanged. Inferred, never written.
    Writes(BTreeSet<(Symbol, Resource)>),
}
pub fn frame_of(footprint: &Footprint) -> Frame;

pub struct Obligation {
    pub key: DefHash,
    /// `<module>.<def>` for a clause, `<module>.<label>` for a law.
    pub owner: Symbol,
    pub kind: ObligationKind,
    pub span: Span,
    pub frame: Frame,
    pub binders: Vec<LawBinder>,
    pub guarded: bool,
    /// `{}` or `{sim.read}`.
    pub footprint: Footprint,
}
pub enum ObligationKind { Ensures { index: usize }, Law }

pub struct Counterexample {
    pub bindings: Vec<Binding>,      // shrunk
    pub original: Vec<Binding>,      // as generated
    pub shrinks: u32,
    pub root: u64,
    pub case: u32,
    pub race: Option<Race>,
    pub sim_seed: Option<Seed>,
}
pub struct Binding { pub name: Symbol, pub ty: Type, pub rendered: String }

pub struct Vacuity { pub guard: Span, pub kind: VacuityKind }
pub enum VacuityKind { ProvedUnsatisfiable, NoCaseKept { generated: u32 } }

pub enum Gap {
    UnhandledEffect(Footprint),
    Ungeneratable { param: Symbol, ty: Type },
    Raised { bindings: Vec<Binding>, diagnostic: Diagnostic },
    /// A full case budget kept nothing and the guard **does** admit a value, one
    /// of which is carried. Added after an audit found `E0420` being reported —
    /// as an error, failing the run — for guards that admit values a sampled run
    /// merely never drew.
    GuardNotSampled { generated: u32, witness: Vec<Binding> },
}

pub struct Coverage {
    pub definitions: usize,
    /// Carries an `ensures` that holds, or is named **directly** by a law that
    /// holds. `requires` alone does not cover; a refuted, vacuous or
    /// unattempted obligation covers nothing; reachability is not coverage.
    pub covered: usize,
    /// Program-wide names, sorted. The exact surface where review still costs
    /// what it costs today.
    pub uncovered: Vec<Symbol>,
    pub by_tier: BTreeMap<Tier, usize>,
}

pub struct ProvePlan {
    pub cases: u32,          // default 200
    pub roots: Vec<u64>,     // ascending, deduplicated
    pub prove_budget: u32,   // static inference steps per obligation, default 10_000
    pub shrink_budget: u32,  // candidate evaluations, default 500 — NOT in the digest
    pub sim: ply_eval::Plan, // for a concurrency law
}
impl ProvePlan { pub fn digest(&self) -> [u8; 32]; }

pub fn prove_key(key: DefHash, plan: &ProvePlan) -> DefHash;
pub fn result_key(key: DefHash, tier: Option<Tier>, plan: &ProvePlan) -> DefHash;
```

### What qualifies for `proved` — implement exactly this

An obligation is `proved` iff a decision procedure over this fragment, and
nothing outside it, answers **valid** for `guard ⟹ body` with every binder a
fresh symbolic constant.

1. **Linear integer arithmetic.** `+`, `-`, unary `-`, multiplication where at
   least one factor is an integer **literal**, and the six comparisons. `x * y`
   with both symbolic, `/` and `%` are **not** arithmetic — they are
   uninterpreted terms. `x / 2 * 2 == x` must not be proved. The prover's terms
   are mathematical integers, and rule 7 is what makes that a statement about
   Ply rather than about ℤ. The `i64` boundary was previously a *disclosed
   unsoundness* mitigated by the generator's `i64::MIN` / `i64::MAX` draws; that
   mitigation could not fire — the static tier answers before any case is drawn,
   and checked arithmetic surfaces the divergence as a raise rather than as a
   refutation — so it is retracted.
2. **Propositional structure**: `&&`, `||`, `!`, `if` at `Bool`, by case split.
3. **Case analysis over ADTs**: split on the outermost constructor, fields become
   fresh symbolic constants. Exhaustive and terminating for recursive types too,
   because the split is over the constructor set rather than the value space —
   which is exactly why it reaches depth 1 and no further.
4. **Structural equality and congruence closure**: constructors injective and
   distinct, record projection reducing, everything else an uninterpreted
   function symbol closed under congruence.
5. **Bounded unfolding**: a call to a definition that is **not** in a recursive
   SCC may be inlined to depth `UNFOLD_DEPTH`. A member of a recursive SCC is
   **never** unfolded. The SCC data is already computed by `ply-hash` for
   component hashing.
6. **Exhaustive enumeration** of a finite domain of at most `ENUMERATION_BOUND`
   points, evaluated at every point. A type is finite iff its constructor graph is
   acyclic and every field type is finite. Ground evaluation is this with one
   point, and it is a proof.
7. **Definedness**, which the six above are conditional on. A proof is issued
   only once the prover has also decided that **every input satisfying the guard
   has an answer**: each arithmetic result is in `[i64::MIN, i64::MAX]`, each
   divisor is nonzero and not `(i64::MIN, -1)`, and every call this prover did
   not inline is refused outright — a member of a recursive SCC is never inlined
   and M8 has no termination checker, so `spin(x) == spin(x)` is a theorem about
   a total symbol and not about `fn spin(x) = spin(x)`. The requirement is
   conditioned on the path it was reached under, and a guard's own requirements
   may not assume the guard. Failing to discharge one is `Unknown`, never a
   refutation. ADR 0007 §5.1(g) is the full statement.

Everything else is **inconclusive**, and:

> An inconclusive attempt reports `property`. Never `proved` — and never
> `Refuted` either: a model over uninterpreted symbols need not correspond to any
> Ply value, so the static side proves or shrugs and the property tier does the
> refuting, with a value it actually ran.

Inconclusive covers a term outside the fragment, an unclosed case split, a spent
`prove_budget`, and a refused unfolding.

### The tier outcomes that are not tiers

| outcome | when | code | exit |
| --- | --- | --- | --- |
| `Vacuous` | the prover showed the guard unsatisfiable, **or** a property run kept zero of a full case budget *and* a directed search over the guard's own literals also found no value it admits | E0420 | 1 |
| `Unattempted(UnhandledEffect)` | checking an `ensures` means calling the definition, and its footprint needs a handler nothing supplies | W0604 | 0 |
| `Unattempted(Ungeneratable)` | a parameter of a type the generator cannot inhabit | W0604 | 0 |
| `Unattempted(Raised)` | an evaluation raised: a runtime error, a division by zero, the recursion limit. A spec that raises is not false | W0604 | 0 |
| `Unattempted(GuardNotSampled)` | a full case budget kept nothing and the guard does admit a value, which is carried. The search missed the domain; the spec is not at fault | W0604 | 0 |

A `Vacuous` obligation is always a defect in the spec: `guard ⟹ body` with an
unsatisfiable guard is valid and says nothing, so a system that reported it
`proved` would turn a typo into a proof of everything. An `Unattempted`
obligation is a reported gap and does **not** fail the run — making it one would
mean a spec could never be attached to an effectful definition — and it counts as
**uncovered**.

### Concurrency laws

A law whose body's row is `{sim.read}` is discharged by execution. It is
`proved` **iff every one of these holds**, and `property` otherwise:

1. `plan.sim.mode == SimMode::Dpor`;
2. `Exploration::exhaustive`;
3. `!Exploration::exhausted`;
4. `Exploration::failure.is_none()`;
5. **and the value domain was covered**: no binders, or every binder's type is
   finite and enumeration ran over all of them.

Condition 5 is the one an implementer will drop and dropping it is this
milestone's worst available defect: `exhaustive: true` is a claim about
schedules, and a law over `n: Int` ranges over 2⁶⁴ values. The two coverage
claims are independent and `proved` needs both. The certificate rule is
`ExhaustiveInterleaving`, deliberately distinct so an audit can find every
execution-derived proof and check it against 1–5.

Everything else about the region is ADR 0006's, unchanged: machine-only under
`--engine treewalk` (E0504), no nesting (E0416), no escaping `Task` (E0413),
stuck is E0414, a divergent replay is E0415.

### Caching

| discharge | written under |
| --- | --- |
| `Held` at `Proved` | the bare obligation key |
| `Held` at `Property` or `Example` | `prove_key(key, plan)`, and **never** the bare key |
| `Refuted`, `Vacuous`, `Unattempted` | nothing |

```
prove_key(key, plan) = blake3( b"ply.prove.key.1" ‖ key
                             ‖ cases_le_u32 ‖ prove_budget_le_u32
                             ‖ roots_len_le_u32 ‖ roots_le_u64*
                             ‖ sim_plan_digest )
```

`shrink_budget` is not in the digest: it can only change a counterexample's
minimality, and failures are never cached.

The asymmetry is the whole operational value of `proved`. A sampled discharge is
a claim about the plan that sampled it, so reading it under a wider plan would
let `--prove-cases 10` satisfy a run that asked for a thousand. A proof is a
claim about all inputs satisfying the guard, so it is valid under every plan and
costs nothing forever.

### Generation and shrinking

Generation is deterministic, from counter-mode BLAKE3, **keyed by the obligation
as well as the root**:

```
draw(root, obligation_key, counter) =
    blake3( b"ply.gen.stream.1" ‖ root_le_u64 ‖ obligation_key ‖ counter_le_u64 )[0..8]
```

Without the obligation in the key, adding a law would shift every later law's
cases, so an unrelated edit would change which counterexample a failing
obligation reports — ADR 0006 §4.2's argument for two separate streams, applied
here.

`Int` draws with edge bias including `0`, `1`, `-1`, `i64::MIN` and `i64::MAX`.
`List` and `String` are length 0..16 biased small. An ADT draws a constructor by
index and, past `GEN_DEPTH`, only constructors with no recursive field, so
generation terminates. A function value is a member of a fixed family — the
constants of the return type, plus a pure total derivation from the argument's
rendering — every member printable, so a counterexample naming a function names
something a reader can act on. A type variable is monomorphised to `Int` and
recorded.

Shrinking has a fixed candidate order per type (`Int` toward 0 by halving;
`List` empty, halves, single removals, then elementwise; ADT toward a recursive
field, then a lower-index constructor, then fieldwise; and so on), and two
requirements that are the whole of its honesty:

1. **A shrunk value must still falsify the obligation** — every candidate is
   re-evaluated, and no monotonicity is assumed.
2. **A shrunk value must still satisfy the guard** — a candidate outside the
   domain is a counterexample to a different claim.

Termination is structural: every type has a saturating `size(v) -> u64`, a
candidate is accepted only if its size strictly decreases, and the walk is
greedy. `--shrink-budget` bounds wall clock, never correctness. Two runs over one
refutation produce byte-identical shrunk bindings, and `Counterexample::original`
keeps the un-shrunk value because "shrank from 400 elements to `[0, 1]` in 11
steps" is the sentence that says the space was searched.

### `ply-cli`

```
ply prove [PATH]              discharge every obligation and report its tier
ply review --changed          what changed, whether its spec changed, whether it still holds
ply review --accept           record the current definition and spec hashes as reviewed
```

`ply prove` flags: `--json`, `--explain`, `--filter <substring>`, `--jobs <n>`,
`--no-cache`, `--prove-cases <n>`, `--prove-roots <n>`, `--prove-budget <n>`,
`--shrink-budget <n>`, plus `--sim`, `--sim-budget` and `--seed` for concurrency
laws.

Exit `0` when every obligation is `Held` or `Unattempted`; `1` on any `Refuted`
or `Vacuous`; `2` on a compile error. `ply prove` **never calls
`observe_definitions`** — ADR 0004 §4's rule: a definition exercised by an
obligation has not been vindicated as a test subject.

**Coverage is in the default output of both commands, ahead of the results, never
behind a flag.**

```
$ ply prove
   41 definitions · 18 carry an obligation · 23 do not
   26 obligations · 7 proved · 16 property · 2 example · 1 unattempted   (0.42s)

   ✓ proved     ledger.withdraw            ensures #0   linear arithmetic · 2 unfoldings · 41 steps
   ✓ proved     "transfers conserve value"              exhaustive over 12 interleavings
   ✓ property   ledger.fee                 ensures #0   200 cases · 0 rejected
   ✓ example    ledger.settle              ensures #0   7 cases kept of 200 · guard rejected 193
   ~ unattempted ledger.post               ensures #0   performs {db.write[accounts]}: no handler

   ✗ refuted    "reverse is an involution"                            src/list.ply:41:1
       forall (xs: List<Int>)  →  xs = [0, 1]
       shrank from [4, 9, 2, 7, 1] in 6 steps · root 41 · case 118

   1 refuted, 25 held (0.42s)
```

`ply review --changed` reports, per changed definition, whether the
implementation changed, whether the spec changed, and whether the obligations
still hold. The five rows are the milestone's argument:

| implementation | spec | what a reviewer does |
| --- | --- | --- |
| changed | unchanged | read the obligations. **The cheapest review in the system.** |
| unchanged | changed | read the spec diff, and nothing else. |
| changed | changed | read both; the tier says how much the machine checked. |
| unchanged | unchanged | nothing to review. |
| either | **none** | read the implementation, exactly as today. |

A row is reached only when the definition carries at least one obligation that
**holds**. A definition whose only obligation is a gap falls to the last row and
counts as unspecified: a claim the machine could not establish is not evidence,
and the advice has to agree with the coverage line or one of them is lying.

The baseline is what a human last **accepted**, not what a machine last ran, and
`ReviewRecord` is keyed by the definition's program-wide **name** — the same trade
ADR 0004 makes for `PassRecord`, because the key has to be the thing that does not
move when a hash does. Renaming loses a baseline, which costs one re-read and
never a false "unchanged".

`--json` on both carries `"schema_version": 1`.

### New diagnostic codes — landed

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0417 | `EFFECT_IN_SPEC` | a `requires`/`ensures`/`where` row is not empty, or a law body's row is not a subset of `{sim.read}` | the program's |
| E0418 | `UNQUANTIFIABLE_TYPE` | a `forall` binder's type has no generator, is a function type with a non-empty row, or mentions an effect-row variable | the program's |
| E0419 | `OBLIGATION_REFUTED` | a counterexample was found | the program's |
| E0420 | `VACUOUS_OBLIGATION` | the guard admitted no values | the program's |
| W0604 | `OBLIGATION_NOT_DISCHARGED` | `Unattempted`, with the `Gap` in the note | nobody's; it is a gap |

Reused rather than invented: a non-`Bool` clause is `TYPE_MISMATCH`; `result` in
a `requires` is `UNKNOWN_NAME` with a note; a parameter named `result` beside an
`ensures`, and two laws with one label, are `DUPLICATE_DEFINITION`. E0419 and
E0420 join `E0501`/`E0502`'s row: the program is at fault and `Failure::defect` is
`false`.

### Versions

`PROVER_VERSION` starts at `0.1.0`. `FRONTEND_VERSION` and `BODY_ENCODING` bump
for the law discriminant in the AST and the normalizer, and for `specs` in
`SourceFingerprint`. `RUNTIME_VERSION` does **not** bump: the evaluator gains no
semantics, because a spec is evaluated by the same machine running the same
programs.

### Required tests

The ADR's numbering; fifty-six of them. These are the ones whose absence would
let the milestone ship a wrong `proved` rather than merely an incomplete one:

- **18**: `Discharge::tier()` returns `Proved` only for `Evidence::Proof`, and
  no other constructor yields it.
- **19**: every `Certificate` over the corpus names only fragment rules and has
  `guard_satisfiable: true`.
- **20**: a spent step budget reports `property`, never `proved` and never
  `refuted`.
- **21**: `x / 2 * 2 == x` is not proved.
- **22**: `reverse(reverse(xs)) == xs` is `property` — a recursive definition is
  never unfolded.
- **33**: **the differential tier audit.** Every corpus obligation reported
  `proved` survives 1,000 sampled cases across 8 roots; a refutation is a defect
  in Ply, classified like E0415 and never bisected. This is `--engine both` for
  the prover and it exists for the same reason.
- **35**: a concurrency law with one `Int` binder is `property` even at
  `exhaustive: true`.
- **39**: a `property` discharge is never written under a bare obligation key.
- **12**: adding, editing and deleting a spec each select zero tests and change
  zero definition hashes.
- **45–46**: a shrunk counterexample still falsifies **and** still satisfies the
  guard, and two runs agree byte for byte.
- **49–52**: coverage is in the default output, `requires` alone does not cover,
  reachability is not coverage, and a refuted or unattempted obligation covers
  nothing.

Plus one `tests/fixtures/` entry per new code, as every milestone owes.

### Not in M8

- **A general-purpose theorem prover.** The fragment above is the whole of it.
- **An SMT integration.** No Z3, no CVC5, no external solver, ever. A solver is a
  trusted oracle whose version changes the answer, and a `proved` label must be
  reproducible from the definition set alone — the argument that put counter-mode
  BLAKE3 in ADR 0006 §4.2 instead of a PRNG crate.
- **A termination checker.** An evaluation that hits `DEFAULT_MAX_CALLS` is
  `Unattempted { Raised }`, never `proved` and never `refuted`.
- **Induction**, well-founded recursion, lemmas or proof hints. This is what puts
  every claim about a recursive definition over unbounded data at `property`, and
  it is the largest restriction on reach.
- **Quantifier alternation.** There is no `exists`.
- **Call-site precondition checking.** `requires` is a **filter on the domain of
  the `ensures` clauses beside it**, not a contract checked at every call. A
  caller that violates one is not diagnosed. A reader of a Ply spec must not read
  `requires` as "the compiler enforces this".
- **Specifying effects.** An `ensures` cannot say what a definition did to a
  resource, because a spec may not name mutable state. **This is the largest gap
  in the milestone.** Closing it needs a pure term denoting a resource's contents
  and a model of the resource behind it.
- **Handler-parametric laws.** A handler is syntax, not a value; ADR 0007 §3.2
  lists the four things that would change.
- **Bounded integer arithmetic**, runtime contract checking, spec-derived code,
  refinement types, dependent types, and a `--coverage` flag.
