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
/// Every name a module declares, as a `Binding`. Parallel to
/// `Resolved::declarations`, and what a *qualified* lookup hands a reference
/// back from — a `binder::name` denotes a declaration in another module, which
/// is therefore in no scope at all.
pub struct Bindings {
    pub values: IndexMap<Symbol, Binding>,
    pub types: IndexMap<Symbol, Binding>,
    pub effects: IndexMap<Symbol, Binding>,
}
pub struct Scope {
    pub module: ModuleName,
    pub modules: IndexMap<Symbol, (usize, Span)>,   // binder -> module index
    /// Modules this file imported names from **without** binding them as a
    /// module, keyed by the binder such an import would not introduce. Kept only
    /// so that `orders::place` after `import store.orders (place)` can say why
    /// `orders` is not in scope.
    pub selective: IndexMap<Symbol, (usize, Span)>,
    pub values: IndexMap<Symbol, Binding>,
    pub types: IndexMap<Symbol, Binding>,
    pub effects: IndexMap<Symbol, Binding>,
}
pub struct Resolved {
    pub scopes: Vec<Scope>,                    // parallel to Program::modules
    pub declarations: Vec<Declarations>,       // parallel to Program::modules
    pub declared: Vec<Bindings>,               // parallel to Program::modules
    pub index: IndexMap<Symbol, usize>,        // module name -> module index
    pub order: Vec<usize>,                     // dependency-first; acyclic
}
impl Resolved {
    pub fn scope(&self, module: usize) -> Option<&Scope>;
    pub fn index_of(&self, name: &ModuleName) -> Option<usize>;
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
impl<'a> Machine<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved, check: &'a CheckOutput) -> Self;
}

// ply-test — three later parameters have been appended; the shipped signature is
// what is written here. `audit_backend` came with `--audit-backend`, `search`
// with M7's plan and `hosts` with W1's binding, each of which a caller has to
// state per run rather than per crate.
pub fn run(selection: &Selection, program: &Program, resolved: &Resolved,
           check: &CheckOutput, hashes: &HashOutput, store: &mut Store,
           audit_backend: bool, search: Search, hosts: Hosting<'_>) -> RunReport;
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
mod effect_set;   // private: W3's `effect set` expansion runs inside the parser

// Re-exported from `parser` at the crate root.
pub fn parse(source: SourceId, text: &str) -> Result<ast::Module, Vec<Diagnostic>>;
pub fn parse_module(source: SourceId, name: ModuleName, text: &str)
    -> Result<ast::Module, Vec<Diagnostic>>;
pub fn parse_program<'a>(inputs: impl IntoIterator<Item = (SourceId, ModuleName, &'a str)>)
    -> Result<ast::Program, Vec<Diagnostic>>;
/// The recovering form: as many parse errors per run as it can find.
pub fn parse_recovering(source: SourceId, name: ModuleName, text: &str)
    -> (ast::Module, Vec<Diagnostic>);
/// One expression, for a snippet and for `ply-test`'s own harness.
pub fn parse_expr(source: SourceId, text: &str) -> Result<ast::Expr, Vec<Diagnostic>>;
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
withRegion:= "with_region" "[" IDENT "]" block
```

Precedence, loosest to tightest: `||`, `&&`, comparison (`== != < <= > >=`),
`++` (string concat), `+ -`, `* / %`, unary `- !`, application / field access /
indexing. `if`, `match`, `handle`, `with_cell`, `with_region`, lambda
(`|x, y| expr`) are primary expressions.

`with_region` is **contextual**, recognized only immediately before a `[`,
exactly as `with_cell` is. `lexer::is_ident("with_region")` stays true and no
`Kw` is added, so a program that already binds the name is unaffected.

Comments are `//` to end of line. Integers are decimal, optionally `_`
separated. Strings are double-quoted with `\n \t \\ \" \r \0` escapes.

Recover from errors where cheap — report several parse errors per run rather
than stopping at the first. Every node must carry a real span.

---

## ply-core

Every struct in this block has grown fields since, each introduced by the section
that added it: `module` / `simple_name` and `CheckOutput::modules` under Modules
above; `OpInfo::scheme` under M7; `SpecInfo`, `LawInfo` and `CheckOutput::laws`
under Specs; `DefInfo::performed` and `row_aliases` under W3. Two more are
carried by no block and are recorded here: **`DefInfo::constraints:
Vec<DefConstraint>`**, the `where derivable(D, a)` clauses sorted and
deduplicated as the hash encodes them, which is part of the published signature;
and **`ModuleInfo { name, source, items, imports }`**, the value of
`CheckOutput::modules`. Every one of these structs also carries a `span`.

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
  are prelude builtins that perform those atoms, and `cell_update` performs both
  (plus its function's row).
- `with_region[r] { body }` (ADR 0017) opens a lexical allocation scope
  branded `r` and discharges the same two atoms at *its* boundary. A
  `with_cell[r]` written under a region of that name allocates **into** it: the
  cell may outlive the `with_cell`'s own braces and is discharged and checked at
  the region instead, which is why a `with_cell` written before regions existed
  does not move. Two regions of one name in scope at once is `E0447`.
- A value branded `r` that would outlive the region is `E0446`, reported where
  it would escape. The check runs on **resolved** types, so an alias hides
  nothing, and it reads a function's effect row as well as its shape, which is
  how a closure that captured a branded value is caught. A variant field
  declared as a concrete `Cell` is `E0446` at the declaration: a variant's field
  types are converted once for the whole program, so a brand stored in one has
  nowhere to appear.
- A handle into a region — a `Cell`, a `Task`, or a continuation, reached through
  any data constructor, a `Secret`'s payload or a closure's captured environment
  — that crosses a runtime boundary where no type is left to check is `E0449`,
  raised by the evaluator and naming the handle, the route to it and the
  boundary. Three boundaries are checked: a host operation's argument, the value
  a host handler or runtime answers with (inline, `block_on` or a token the
  scheduler resolves), and an argument handed to an entry point from outside the
  program. `E0449` joins `RESERVED_CODES` — it is the machine's verdict about its
  own memory, so a handler may not mint it. Both engines check at the same point
  with the same message, or the refusal would itself be an `--audit-backend`
  divergence. `ply_eval::escape` documents what every other boundary ADR 0017
  names does instead, and which one route stays open.
- **A region's slots go back at its lexical close**, for both
  kinds. What the kind decides is a claim about that close rather than whether it
  happens: a close no live continuation can reach truncates the arena, and one a
  continuation captured across the region can still reach retains its slots until
  that continuation dies (ADR 0017, §4). Meaning is unchanged either way —
  state is threaded, resumption *n* observes resumption *n−1*'s writes — because
  reclamation is decided by reachability and not by the region kind. An entry
  point's end closes whatever the run left open and abandons every claim, which
  is what keeps one test's cells out of the next one's arena.
- A `/ {...}` annotation is an upper bound: inference must produce a subset, and
  the annotation becomes the published signature. Violation is `EFFECT_NOT_PERMITTED`.
- Every parameter type and return type on a top-level `fn` is **written**. An
  omitted one is `MISSING_SIGNATURE` (`E0126`), reported after the component's
  numerics settle so the diagnostic can name the type inference would have
  given. A definition a `derive` generated is exempt: nobody wrote it. This is
  the deliberate asymmetry with the row above — a row is derived from what a
  body calls and stays inferred, a type is chosen and is written.
- A local `let` binds **monomorphically**. Generalization happens at a top-level
  definition and nowhere else.
- An arithmetic or ordered-comparison operand whose numeric type nothing
  determines is `NUMERIC_UNDETERMINED` (`E0210`). There is no defaulting rule.
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
pub mod body;     // the definition-body encoding; see Cache storage below

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct DefHash(pub [u8; 32]);

impl DefHash {
    pub fn to_hex(&self) -> String;          // 64 chars
    pub fn short(&self) -> String;           // first 12 hex chars, for display
    pub fn from_hex(s: &str) -> Option<DefHash>;   // exactly 64 chars, or None
}
impl std::fmt::Display for DefHash;          // == short()

pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    /// `type` and `effect` declarations, which `defs` deliberately omits — only
    /// a `fn` is a definition a test can be selected on. They are hashed all the
    /// same, because a cached interface has to be keyed by one.
    pub decls: IndexMap<Symbol, DefHash>,
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
  `blake3(component_hash ‖ index_le_u32)`. The index is **not** source position —
  moving a definition would change its hash — but the class partition refinement
  assigns: start with every member in one class, re-encode with each
  intra-component reference written as the referent's current class, and split
  until the partition settles, then re-encode once more so the labels the bytes
  mention are the labels the members are filed under. The component's payload is
  one encoding per class, laid out in class order. Stopping a round early would
  leave every reference reading `class 0`, under which `f → g, g → h, h → f` and
  `f → h, h → g, g → f` are one definition set.
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
    pub reuse: bool,                     // a `reuse fn`: gate 1 knows without a parse,
                                         // because the promise is checked whole-program
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

**Gate 2 — definition level.** Inside a file that did change, recheck a
definition whose **own hash** moved — its normalized form with references by
name — and any definition a rechecked one's **interface hash** turns out to have
moved under. The interface is the published scheme, footprint and constraints,
which are everything a caller can observe; the footprint is in it because effect
rows are inferred, so a body edit that adds a `perform` still reaches callers.

That runs as waves: check what moved, compare each rechecked definition's fresh
interface against the stored one, admit the callers of only those that differ,
repeat to a fixpoint. Only the final wave's `CheckOutput` and diagnostics may
escape — an intermediate wave can hand a caller a stale interface.

`DefHash` remains the key for the **test cache** and for selection, and is
unchanged. It is transitive by construction, which is the right question for
"must this test re-run" and too strong a question for "must this definition be
rechecked": written signatures mean a caller's type cannot depend on a callee's
body. `docs/adr/0002-incremental-frontend.md` §"Early cutoff" carries the
reasoning and the two wrong readings that look sound.

Gate 1's key differs for a reason that must not be optimized away: **a `DefHash`
cannot be computed without parsing**, so gate 1 has to key on raw file content.
Gate 1 is conservative about formatting; gate 2 is conservative about a name — a
renamed callee moves its callers' own hashes and costs a recheck, never a wrong
answer.

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
///
/// It is declared as `ply_store::schema::fingerprint` and re-exported from the
/// crate root under this name; both spellings are the same function.
pub fn schema_fingerprint() -> ContentHash;   // == schema::fingerprint

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

**Superseded.** `Value` is stated in full under "The control stack and the world"
below and grows again under W1, W2 and W5; `Cell(Rc<RefCell<Value>>)` in
particular is gone, and `Interp` itself is gone — see §"Deleted with the
tree-walker". The block is the M2 record.

```rust
pub enum Value {
    Int(i64), Bool(bool), Str(Arc<str>), Unit,
    List(List),                             // ply-eval::list — a radix trie with its newest
                                            // leaf held apart (ADR 0034); cheap clone
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
- **A loop is not a recursion.** `map`, `filter`, `fold`, `iterate`, `map_fold`,
  `bytes_position`, `cell_update` and `map_update` are driven by a step protocol — `Step::Apply` answered by
  a `Frame` the machine pushes and pops — so each costs **depth 1** however many
  rounds it runs. A driver that nested would make the bound a function of how a
  loop is spelled, which is the defect ADR 0005 removed tail-call elision to
  prevent. ADR 0022.
- Prelude builtins: `assert`, `assert_eq`, `len`, `push`, `list_at`, `map`,
  `filter`, `fold`, `iterate`, `range`, `int_to_string`, `string_concat`,
  `cell_get`, `cell_set`, `cell_update`, `panic`, `wrap_add`, `wrap_sub`, `wrap_mul`,
  `byte_of_int`, plus the `Bytes` and text builtins in the host-boundary section
  below.
  The three `wrap_*` are the only arithmetic in the language that cannot raise;
  `+`, `-` and `*` stay checked and `<<` is the only *operator* that discards
  rather than raising (ADR 0033 §2.2). `byte_of_int` is `bytes_at`'s inverse and
  raises outside `0..=255`.
  A failing `assert`/`assert_eq` is `ASSERTION_FAILED` with a structured
  expected/actual message.
- An evaluator must be usable from a worker thread. If `Value` holds `Rc`, keep
  each interpreter and every value it produces confined to one thread and do not
  implement `Send` for it — the scheduler hands each worker its own.

---

## ply-test

**Superseded in three places.** `select` takes a fourth argument, a `Plan` — M7
below, and the breaking change is deliberate. `run` takes the nine-argument form
under "Changed signatures" above. `Failure` and `RunReport` are restated in full
under "Machine-shaped failure" below. The block is the M2 record.

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

`ply check --types` prints each definition's effect row on its own line under
its type, wrapped at 80 columns and never at the terminal's width, so the output
is diffable. A pure definition prints no row at all. The row is always the
**expansion**: an `effect set` name appears nowhere without `--explain`.

`ply check --types --explain` adds, per module, the `effect set` table — name,
expansion, and how many definitions use it, directly or through another set —
and, per definition, the sets its row was written with, the row its body
performed, and the difference. Its bytes may not depend on the cache, so the run
completes any parse gate 1 skipped before printing; the front-end report above
it still says what the gates actually decided. `--json --explain` carries the
same data as `effect_sets` per module and `written_as` / `performed` /
`declared_not_performed` per definition, and those fields are absent without the
flag rather than partially filled. `ply prove --explain` names the same
difference for a definition carrying an obligation, where it is a weakened frame
condition rather than a scheduling cost.

`--tls NAME=CERT,KEY` is repeatable and accepted by `ply run`, `ply test` and
`ply hosts`. It requires `--host`: credentials configure a binding, and one that
would be silently ignored reads as a run that served TLS. A malformed argument
is a usage error naming the form; material that does not load is `E0430` naming
the file, before anything runs. `ply hosts --host` grows a `transport` block
naming the TLS library, its provider, the protocol versions and the ALPN
protocols whenever the listing resolves a TLS handler, and a `credentials` block
naming each credential with an abbreviated fingerprint — `--json` carries the
whole fingerprint. The digest covers the credential *names*, the provider and
the library version, and not the fingerprint: a certificate renewal must not
move it, and adding or removing a credential must. A program that reaches no TLS
handler prints no transport block and its digest is unchanged.

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

/// Which namespace a change is in. Added with `PassRecord::decls`, and for the
/// same reason: a `fn` and a `type` may share a name, so a change set keyed by
/// name alone silently drops one of them.
pub enum Ns { Value, Decl }
pub struct DefKey { pub name: Symbol, pub ns: Ns }

pub struct Change {
    pub name: Symbol,
    pub ns: Ns,
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
pub struct Cluster {
    pub members: Vec<Symbol>,
    /// The same members with their namespaces, which is what a hybrid flips:
    /// `members` deduplicates a name that is both a `fn` and a `type`.
    pub keys: Vec<DefKey>,
    pub reason: FusionReason,
}

pub struct Delta { pub test: Option<Change>, pub changes: Vec<Change>,
                   pub clusters: Vec<Cluster>,
                   /// Changes that could not be told apart from a hash that
                   /// merely moved. Non-zero is the same disqualification an
                   /// unresolved *trial* is: the answer may be right, but the
                   /// run may not call it minimal.
                   pub unclassified: usize }
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

### Required of other crates — landed, with two shapes corrected

```rust
// ply-store — the baseline. One record per test key, overwritten on each pass,
// never written for a failing or nondet test.
pub struct PassRecord {
    pub test_hash: DefHash,
    pub closure: BTreeMap<Symbol, DefHash>,
    /// The `type` and `effect` declarations, kept apart from `closure` because a
    /// `fn` and a `type` may share a name: one map for both would record
    /// whichever the writer preferred and drop the other, so an edit to the
    /// loser would be invisible to every later bisection.
    pub decls: BTreeMap<Symbol, DefHash>,
}
impl Store {
    pub fn pass_record(&self, key: &Symbol) -> Option<&PassRecord>;
    pub fn put_pass_record(&mut self, key: Symbol, record: PassRecord);
    /// Bodies, per ADR 0003 — name-erased and hash-linked, so a hybrid mixing
    /// two namespaces needs no module layout invented for it. The stored type is
    /// `DefBody` and the entry is materialized from a mapped byte range, so it
    /// is answered by value: `Option<Arc<DefBody>>`, never `Option<&Definition>`.
    /// "Cache storage" above is the authority on it.
    pub fn body(&self, hash: DefHash) -> Option<Arc<DefBody>>;
}
```

`prune` gains a second retention root: every hash reachable from a surviving
`PassRecord::closure`. Dropping those silently downgrades every future bisection
to `no_bodies`.

`ply-eval` gains the tracer — hooked where a named closure is applied and at
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
artifact of ADR 0004 `failures[].suspects` becoming an array of objects is a
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
    List(List),
    Record(Arc<BTreeMap<Symbol, Value>>),
    Ctor { name: Symbol, args: Arc<Vec<Value>> },
    Closure(Arc<Closure>),
    /// A key into the `World`, not a pointer into it.
    Cell(CellId),                    // now `Cell(Slot)`; see "Test isolation
                                     // under regions" below — ADR 0017 replaced
                                     // the world with a region arena, and a
                                     // `Slot` is an index plus a generation.
    /// Callable with exactly one argument — the value the `perform` it was
    /// captured at should have produced. Any other count is `ARITY_MISMATCH`.
    Continuation(Rc<Continuation>),
}

impl Value {
    pub fn as_cell(&self, span: Span, what: &str) -> Result<CellId, Diagnostic>;
    // now `-> Result<Slot, Diagnostic>`
}
```

`Value` has since grown `Float`, `Decimal`, `Bytes`, `Map`, `Task` and `Secret`;
each is introduced by the milestone section that added it, below.

`Cell(Rc<RefCell<Value>>)` is gone, and with it the "cell cannot be read while it
is already borrowed" runtime error — a `RefCell` reentrancy failure cannot happen
to a map entry. `values_equal` compares cells by `CellId`, which is the same
identity comparison `Rc::ptr_eq` gave. A `Continuation` compares like a closure:
`RUNTIME_ERROR`, "cannot compare functions for equality".

### `ply-eval::world` — landed, then **deleted**

> **Nothing in this subsection exists.** ADR 0017 replaced the persistent world
> with a region arena, and the module, the type and the id all went with it.
> There is no `ply_eval::world`, no `World` and no `CellId` in `crates/`. The
> block below is kept as the record of what the milestone shipped and of what
> each name became; "Test isolation under regions" at the end of this file is the
> live contract. The mapping, in the file's usual form:
>
> - `ply_eval::world` → `ply_eval::arena` and `ply_eval::task_regions`
> - `World` → `TaskRegions` / `Arena`  (`Fixture` is the seeded pair)
> - `CellId` → `Slot`, an index **and a generation**, so a cell whose region has
>   closed reads `None` instead of aliasing whatever was allocated in its place;
>   `Display` is `"@7.0"`, not `"#7"`
> - `World::fork` → the entry point's region reset; `GroupRegion::open`, which is
>   linear in the fixture where `fork` was one pointer clone
> - `World::high_water` → gone with the fork; nothing replaced it, because
>   sibling forks are what it dated the ids against
> - `World::cells` → `Arena::slots`

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

**What replaced the two paragraphs above.** Sibling id collision is gone, because
there are no siblings: one arena per entry point, slots handed out from a bump
pointer, and a generation on each so a stale slot fails to resolve rather than
aliasing. The fixture is still a pair — `ply_eval::Fixture`, a `TaskRegions` and
a `Value`, reached through `ply_test::region::GroupRegion` — but opening it
replays the fixture's slots into a fresh arena, which is **linear** where
`World::fork` was one pointer clone. That regression is priced in "Test isolation
under regions", required property 7.

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
// Corrected (R4, 2026-08-21) — the block above is what was designed and the
// three lines below are what `crates/ply-eval/src/code.rs` holds. Nothing reads
// this file, so it went stale silently, which is what `CONTRIBUTING.md`
// §"Before you open a change" item 5 is about.
pub struct Node { pub kind: NodeKind, pub span: Span, pub own: Own }
//   NodeKind::Lit carries the Value it denotes, built once at lowering rather
//   than per evaluation — ADR 0019 The Lit stays because
//   `crates/ply-codegen-spike` dispatches on it to pick a Cranelift type.
Lit(Lit, Value),
//   and Stmt::Expr is a struct variant, which is what bit-rotted the spike.
Stmt::Expr { code: Code, dead: bool },
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

### `ply-eval::argv` — landed after this document, R4

Not designed here; recorded so the public surface is complete. A thread-local
free list of `Vec<Value>` in four capacity classes, taken at
`Frame::AppCallee` and given back by `Machine::enter_code` once the arguments
are bound into scope. ADR 0019 is the decision and its module note is the
measurement.

```rust
pub const ply_eval::ARGUMENT_VECTOR_CLASSES: usize;  // = argv::CLASSES
pub(crate) fn take(arity: usize) -> Vec<Value>;
pub(crate) fn give(args: Vec<Value>);
```

The constant is public for one reason and it is worth stating as a contract: the
attribution harness must split its arity histogram at the same number this
module serves, rather than at a copy of it. `give` **refuses a non-empty
vector** rather than emptying it — a caller still holding an argument has not
finished with it, and a pooled buffer holding a `Value` would keep a `Cell` past
the region that reclaims it and park a `Secret` where the next call reads
(ADR 0011). Every access is `try_with`, so a release during thread-local
teardown falls back to the allocator instead of aborting a worker.

### `ply-eval::cont` — landed

```rust
pub enum Frame { .. }                // 30 kinds as shipped; the ADR's table
                                     // describes the 20 M6 landed. Added since:
                                     // `CloseRegion` (ADR 0017), the
                                     // `MapFoldStep` / `BytesPositionStep` /
                                     // `CellUpdateStep` / `MapUpdateStep`
                                     // builtin steps, and ADR 0034's window
                                     // bookkeeping, `Exit` and `Restore`.
pub struct Prompt {
    pub clauses: Rc<Vec<Clause>>,
    /// Program-wide effect names, parallel to `clauses`, resolved where the
    /// `handle` was written.
    pub effects: Rc<Vec<Symbol>>,
    pub ret: Option<Rc<ReturnArm>>,
    /// Per clause, and for the return arm, the values their free variables
    /// were bound to where the handler was installed — copied out of the
    /// installing window at handle entry (ADR 0034).
    pub clause_captures: Vec<Rc<[Value]>>,
    pub ret_captures: Rc<[Value]>,
    pub module: usize, pub span: Span,
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
    ///
    /// `born` is `Machine::host_ops` at the capture, stamped onto the
    /// continuation for W1's linearity rule below — the parameter arrived with
    /// the host boundary and is not optional.
    pub fn capture(&self, segments: usize, born: u64) -> (Continuation, Stack);
    pub fn resume(&self, k: &Continuation) -> Stack;
    /// The `Frame::Call`s pending, O(1) through push, pop, capture and splice.
    pub fn calls(&self) -> usize;
}

pub struct Continuation { .. }       // Clone
impl Continuation {
    pub fn frames(&self) -> usize;
    pub fn segments(&self) -> usize;
    pub fn calls(&self) -> usize;
}
```

`capture` copies **one entry per enclosing handler crossed**, never one per
pending frame. That is the property that makes multi-shot affordable, and
required test 15 pins it.

### `ply-eval::machine` — landed

Written "not implemented" when this section was drafted; the machine is the
default engine as of `RUNTIME_VERSION` 0.4.0 and every entry point below exists.
Two do not: see the note after the block.

```rust
// Revised 2026-08-24. This block declared:
//
//     /// A resource limit on the heap the frames live on. Not the bound a
//     /// runaway recursion hits — that is `limit::DEFAULT_MAX_CALLS`, which
//     /// the machine bounds and which a recursion reaches first, since a call
//     /// costs a frame.
//     pub const DEFAULT_MAX_FRAMES: usize = 1_000_000;
//
// The constant is gone and so is the default. Its last clause was false — a
// call costs one frame, a body costs as many as it pends — and a ceiling that
// fires on a body pending 100+ frames per call is a bound only one engine has.
// `with_max_frames` below is what remains: opt-in, not semantics, and a machine
// carrying one enters no compiled body.

pub enum Progress { Running, Halted(Value) }

pub struct Machine<'a> { .. }
impl<'a> Machine<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved, check: &'a CheckOutput) -> Machine<'a>;
    pub fn for_program(program: &'a Program, resolved: &'a Resolved) -> Machine<'a>;
    pub fn with_max_frames(self, max: usize) -> Machine<'a>;
    pub fn with_max_calls(self, max: usize) -> Machine<'a>;
    /// The atoms this engine performed at the last entry point.
    pub fn trace(&self) -> &Trace;
    pub fn set_base_world(&mut self, world: World);   // deleted with `World`
    pub fn world(&self) -> &World;                    // deleted with `World`
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

`set_base_world` and `world` **do not exist**: they went with `World`. A machine's
cell state is its `Arena`, seeded per entry point from a `Fixture` the runner
holds — `ply_test::region::GroupRegion` above, and `Machine::cells`, which the
`Evaluator` trait `--audit-backend` compares through also names.

The machine has also grown entry points this block predates, each introduced
where its milestone is: `set_seed` / `simulated` (M7), `set_host_binding` /
`set_declared_footprint` / `set_re_executed` / `host_ops` / `host_use` (W1).

The transition rules are ADR 0005 and are normative — in particular:

- `W` is threaded through capture and through resumption **unchanged**;
- a tail-resumptive clause runs on the post-capture stack with a `Frame::Resume`
  pushed for it, so `op(x̄) -> e` is exactly `op(x̄) resume k -> k(e)`;
- a clause body runs on the stack *below* its own handler, so a clause that
  performs the operation it handles reaches the next handler out.

### `ply-eval::Engine` — deleted

`Engine` and `EngineChoice` selected between two evaluators for one milestone.
There is one evaluator now, so both are gone along with `Interp` and `--engine`;
see §"Deleted with the tree-walker". A cached `Pass` is a claim about what the
machine answered on its own, which is what `--backend` implying `--no-cache`
keeps true.

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

**`params_i` and `ret_i` instantiate the operation with its own type variables
rigid**, which is the soundness of the construct rather than a refinement of it.
A clause is written once and answers every perform site there will ever be, and a
row carries atoms and no types — so a `handle` cannot see which `a` a perform
three definitions deeper unified an `-> a` with. Instantiating the clause with
ordinary fresh variables let the *clause* choose: for `read fetch[k](s:
Secret<String>) -> a`, the clause `fetch[k](s) -> s` answered a `Secret<String>`
while the perform site was typed `String`, which is general type confusion and
was also the route a credential was laundered into a `Map` key and its plaintext
read off the key order. Rigid variables say "for every `a`". `E0201` names the
clause and points at the declaration.

The cost is real and is not a bug: an operation whose return type is a variable
its parameters do not determine can no longer be handled at all, because no
clause can produce a `List<a>` out of nothing. Such an operation is unsound
rather than awkward; `examples/store.ply` declared one and was rewritten to
declare what each of its tables holds, with its resource labels and footprints
unchanged.

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

### `ply-test` — landed, then renamed by ADR 0017

Every name in this block was renamed when the forkable world went, and the
**exemption it granted was withdrawn**. "Test isolation under regions" at the end
of this file is the live contract; the mapping is annotated inline.

```rust
/// Effects whose atoms name state living in a `World`. Exactly one at v0.
pub const WORLD_BACKED: &[&str] = &["cell"];   // now `REGION_SCOPED`, and no
                                               // longer an exemption

pub fn is_world_backed(atom: &EffectAtom) -> bool;   // now `is_region_scoped`
/// Every atom is world-backed. The empty footprint is world-isolated.
pub fn world_isolated(f: &Footprint) -> bool;        // now `region_isolated`,
                                                     // widened to "no atom
                                                     // contends" by M7
/// The atoms that can conflict across tests: `f` minus its world-backed atoms.
pub fn shared_footprint(f: &Footprint) -> Footprint; // now drops ambient only
```

`group_by_conflict` colours `shared_footprint`s, not raw footprints. A
world-backed atom conflicts with nothing across tests, because each test's cells
are entries in its own forked world and no reference crosses.

**That last sentence stopped being true.** There is no fork: a `cell` atom
contends like any other atom now, and what buys the isolation is the colouring
plus a region closed at the end of each test. `isolation: World | Shared` is
`Isolation::Region | Shared`, and `Parallelism::region_contended` is the number
this change costs, printed on every run so it cannot go unnoticed.

Each worker forks the base world per test. `Selection` gains, and `RunReport`
reports:

- `isolation: World | Shared` per test, with the atoms that made it shared;
- `isolated: n of m` in the summary and in `--json`.

Required properties, and they are the milestone's measurable claim rather than a
slogan: every world-isolated test is in group 0 for any number of them; adding
*N* world-isolated tests changes the group count by **zero**; the group count
equals the colouring of the shared tests alone. All three still hold, over
`region_isolated` rather than over `world_isolated`.

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
/// recursive program fails `--audit-backend` on every corpus that has one.
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
- **depth of source** — `code::lower`, `Expr::clone`,
  `Infer::infer` and `collect_refs` *grow* the host stack rather than refuse, as
  the parser and the normalizer already do. A bound here would reject on one
  engine a program the front end accepted, which is an `E0503` divergence on
  every corpus with a long operator chain in it.

`Stack::calls()` is the machine's count — the `Frame::Call`s pending, O(1)
through push, pop, capture and splice. A tail call is charged like any other:
eliding it left a tail-recursive runaway unbounded rather than a diagnostic,
which is ADR 0005

### The constant memo — `ply-eval::memo`

A definition with **no parameters**, an **empty published row** and no
`where derivable(..)` constraint is a constant, and the evaluator evaluates it at
most once per `Machine`. The rule reads the published row rather
than the inferred body row: a definition annotated wider than it performs is
left alone, because the annotation is the reviewable artifact and a rule that
disagreed with it would be a rule nobody could check by reading a signature.

Nothing about this is observable in a value, an atom, a trace or a world — that
is the argument for doing it. One thing is: the calls pending underneath a
second reference to a constant, which is why it is in this section and why it
moved `RUNTIME_VERSION` to `0.11.2`. Both engines therefore have to keep the
memo or neither may, or `--audit-backend` reports `E0503` on any program that
reaches a constant from near the bound.

Three rules make it exact:

- **The first completed evaluation wins.** A body that captures a continuation
  and hands it out can be re-entered later with a different resumption value;
  that value is the resumption's and not the definition's, so a filled slot is
  never overwritten.
- **No memo inside an open region**, read or written. The reason is a
  `simulate` region's: a pure definition may open its own `with_cell`, and an
  allocation is an `Access::Alloc` the search depends on; skipping one would
  change what a schedule records, which is what partial-order reduction and
  seeded replay are read off. Outside a region a cell cannot escape the
  `with_cell` that made it — `E0304` — so the allocation is unobservable and the
  substitution is exact. **`Machine::constant` implements this as
  `!self.sims.is_empty()`, which is wider than the reason**: a *production*
  region — the one `task.spawn` opens and keeps open for the life of the
  scheduler — keeps no trail and records no step, and it disables the memo
  anyway. A service whose accept loop spawns therefore memoizes nothing, which
  is measured below and is a defect rather than a rule.
- **No `CheckOutput`, no memo.** `Machine::for_program` and
  `Machine::for_program` evaluates without a check pass and has no published row
  to read, so they remember nothing.

`examples/desk.ply`'s `table()` is what this was measured on: eleven route
patterns parsed from their strings, built once in `route_of` and again in
`health`. Against a control that is the same service with its own nullary
definitions given a dead parameter, with every response byte-identical between
the two:

- **In process, over the twin**, driven alternately in one process at best of 7
  × 512 requests: `/health` **482.6µs → 264.0µs** (2,072 → 3,787 req/s, 1.83x)
  and `/items` **903.9µs → 813.8µs** (1.11x).
- **Served, the real binary over postgres over TLS with `--trace json`**, both
  variants served alternately at concurrency 1, best of 3 (ADR 0011, which
  prices this as one of its seven cheaper levers): `/health` **466.6µs →
  263.5µs** (1.77x) and `/items` **677.0µs → 589.4µs** (1.15x).
- **On the task-per-connection accept loop, nothing:** `/health` 471.3µs against
  470.6µs, `/items` 1,087.7µs against 1,090.6µs — 1.00x either way, because the
  region the spawn opens has already disabled the memo for both variants.

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
`observed_performs` are how `--audit-backend` reads it, and a corpus whose
`footprints_compared` is short of `compared` fails the audit.

### `ply-cli`

```
--backend <spec>       attach a compiled backend to the machine
--audit-backend        also run each test without it, and compare
```

On `ply test`. `--audit-backend` runs each test twice — once with the backend
attached and once without — and compares, per test:

- the `Result<(), Diagnostic>` by **full JSON serialization** — code, severity,
  message, every label with its span, every note. Not "both failed";
- the observed footprint from the tracer;
- the final cell state, as the `(Slot, rendered Value)` sequence from
  `Arena::slots` **and** the two arenas' reclamation, which
  `differential::compare_outcomes` checks beside the contents.

A mismatch **fails the run** with a diagnostic naming the test and both outcomes,
and the blame is the backend's: `Machine::compiled_answer` hands it no route back
into the machine, so a wrong answer is the only thing it can contribute. Never a
warning: a wrong answer is made sticky by the cache.

Three codes partition a failing run, and confusing any two of them costs
something specific:

| code | whose fault | what `ply-test` does |
| --- | --- | --- |
| `E0501` / `E0502` | the program's — an assertion, `panic`, division by zero, the recursion limit, a value past `MAX_VALUE_DEPTH` | attributes it: suspects, bisection, culprit |
| `E0503` `ENGINE_DIVERGENCE` | Ply's — a compiled backend and the machine disagree | `Status::Panicked`, `Skipped::Panicked`, no bisection |
| `E0505` `INTERNAL_ERROR` | Ply's — an evaluator invariant broke | `Status::Panicked`, `Skipped::Panicked`, no bisection |

`Failure::defect` is the observed answer wherever there is one to observe: a run
that watched the evaluator unwind knows something no diagnostic carries. Reading
it off `RUNTIME_ERROR` instead is what made a runaway recursion — a documented
limit, and as bisectable a regression as any assertion — report itself as a
defect in Ply and switch M5 off for the whole class.

`E0503` is the one exception, and only because there is nothing to observe: the
divergence is a comparison the audit *made*, handed back as an ordinary `Err`
rather than as an unwind. Bisecting it would name whichever definition the
disagreement happened to run through.

`--backend` implies `--no-cache` in both directions: a `Pass` in the store is a
claim about what the evaluator answered on its own, and a run that entered
compiled code may neither read nor write one.

`ply test` also prints `isolated: n of m` in its summary.

**`--audit-backend` reports its own coverage.** The oracle cannot compare every
test. A searched test is replayed per interleaving on machines built for the
schedule, so the pair never runs on it; and a test that reaches a host handler is
run once on purpose, because a handler is not a function and a second machine
would send the packet twice. Both exclusions are correct; leaving them uncounted
is not, because `0 failed, n passed` with a backend attached reads as the backend
having answered correctly for *n* tests. So `RunReport` carries
`audit: Option<AuditSummary>` — `{ compared, unaudited }` — and `TestResult`
carries `audited: Option<bool>`, both **absent rather than zeroed** when no
oracle ran. `ply test` prints `audited n of m · k ran unpaired`, and `--json`
carries the same under `audit` and `tests[].audited`.

### Deleted with the tree-walker

Each of these was a workaround for the native stack, and the explicit control
stack is what retired it. All are gone.

| deleted | why |
| --- | --- |
| `Interp`, `Engine`, `EngineChoice`, `--engine` | one milestone of two evaluators, then one |
| `Interp`'s own nesting counter | the bound is `DEFAULT_MAX_CALLS` on `Stack::calls()` |
| `#[inline(never)]` on the `eval_*` arms | they kept the recursive `eval` frame small |
| `E0504` `MACHINE_ONLY_CLAUSE`, `is_machine_only`, `machine_only_clauses` | nothing refuses a clause any more |
| `tests::recursion_to_the_depth_limit_survives_a_one_mebibyte_thread_stack` | it asserted a property that stopped existing, not one that fails |

**`grow()` and the `stacker` dependency stayed, and this table used to say they
went.** They are not the evaluator's: `ply_eval::limit::grow` has about forty
call sites in value comparison, canonicalization, lowering, region-kind
inference, the cost walk and the escape check, and every one of them is a
structural walk over a value or an AST that recurses natively whatever runs the
program. `MAX_VALUE_DEPTH` is 10,000, so a value that deep is exactly what the
segmented stack is for. The other crates' copies — `ply-syntax`, `ply-core`,
`ply-hash`, `ply-store`, `ply-prove` — are front-end recursion and were never in
scope.

The **semantic** limit stays: a runaway recursion is a diagnostic, not an
out-of-memory kill. Its message keeps the phrase "recursion limit" so ADR 0004's
`AssertionKind::RecursionLimit` still classifies it, and it names the innermost
`Call` frames.

### Workspace

`rpds = "1.2.1"` — the persistent `RedBlackTreeMap` a `World` is, parameterized
over the shared-pointer kind so it uses `Rc` and non-atomic refcounts, matching
the existing decision that an evaluator is confined to one thread. It iterates in
key order, which the byte-identical-artifact rule needs.

The dependency stayed when the world went; what it backs now is `Value::Map`
(`ply_eval::Map` is `rpds::RedBlackTreeMap<Value, Value>`, W2 below), for the
same two reasons — a persistent map under `Rc`, iterated in key order.

The control stack does **not** use `rpds::List`. Its links hold the frame
inline, so pushing one costs a single allocation where `List` — which boxes the
value separately from the node — costs two, and popping an unshared link costs
none where `List` cost two more. A push and a pop are the machine's two most
frequent steps, so that is the difference between the machine's profile being
its own work and being the allocator's.

### Required tests

Numbered as in ADR 0005; `(landed)` marks the ones already passing.

1. Every `ply-eval` unit test passes.
2. `--audit-backend` over `examples/`, `tests/fixtures/` and the generated corpus
   reports zero divergences.
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
17. Exceeding a frame ceiling asked for through `with_max_frames` is a
    diagnostic whose notes name the innermost `Call` frames and whose message
    does **not** contain "recursion limit". *(Revised 2026-08-24; it read
    "Exceeding `DEFAULT_MAX_FRAMES` is a diagnostic containing 'recursion limit'
    whose notes name the innermost `Call` frames.")*
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
    /// A stream resumed mid-sequence, for a replay that starts from a recorded
    /// draw count rather than from zero.
    pub fn at(root: u64, domain: Domain, counter: u64) -> Stream;
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
    /// Whether running this plan can drive the entry point more than once. An
    /// upper bound; W1's `E0425` is stated from it.
    pub fn re_executes(&self) -> bool;
    /// Covers every field, length-prefixed. Two plans that search the same thing
    /// have the same digest, or the cache splits on the order a caller happened
    /// to write its flags in.
    pub fn digest(&self) -> [u8; 32];
}
impl Default for Plan;   // Dpor, roots [0], DEFAULT_BUDGET, DEFAULT_STEPS

/// One access a step made. Finer than a `Footprint` in exactly one place: a cell
/// is a *location*, so it is keyed by `Slot` rather than by the `[r]` label
/// several cells may share.
pub enum Access {
    Atom(EffectAtom),
    Cell { id: Slot, mode: Mode },      // was `id: CellId`, before ADR 0017
    /// A `with_cell` took the next slot from the arena's bump pointer — the
    /// world's own counter, before ADR 0017. Allocation has no location to name,
    /// so it is its own kind of access.
    Alloc,
}
impl Access {
    /// Two `Atom`s by `EffectAtom::conflicts_with`; two `Cell`s iff the same
    /// `Slot` and at least one is a `Write`; two `Alloc`s always, since they
    /// take ids from one counter; anything else never.
    pub fn conflicts_with(&self, other: &Access) -> bool;
    pub fn is_write(&self) -> bool;
}
impl Display for Access;   // "db.write[users]", "cell.write[@7.0]" or
                           // "cell.alloc" — a `Slot` renders as index.generation,
                           // where a `CellId` rendered "#7"

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
the seam, which is why `Evaluator` gains nothing: a searched test is replayed
per interleaving on machines the worker never keeps.

The scheduler is a **native prompt**: a delimiter on the M6 stack whose clauses
are Rust. `Segment` gains a native form and `Stack::find_handler` consults both;
capture, splice, deep handlers and the threaded world are all unchanged. A task
is `(TaskId, Continuation, TaskState)`, and resuming one is the transition
ADR 0005 already specifies for applying a continuation.

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

`world_isolated` — since renamed `region_isolated` — widened from "every atom is
world-backed" to "no atom contends", because `sim.read` must not drop every
simulated test out of the `isolated: n of m` number for no reason.
`shared_footprint` dropped ambient atoms with the world-backed ones; under ADR 0017 it drops **ambient only**, since a region label contends. Both are landed,
with tests.

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
37. Under `--audit-backend` a test containing `simulate` runs once and
    `--explain` records it as `searched`.
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

pub struct FnDef { /* … */ pub spec: Vec<SpecClause>, pub reuse: Option<Span>, /* … */ }
```

`reuse` is contextual the same way: it opens an item only when `fn` follows
(after `pub`, if any), its span is kept on the definition for `E0127`'s
secondary label, and normalization erases it as it erases a spec — a promise is
an obligation on the body, not part of what the body denotes.

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
    /// `law/host`, from `LawDef::host` — W4 below. Not in M8; carried here
    /// because this block is otherwise the whole of the shipped struct.
    pub host: bool,
    /// `{}`, or `{sim.read}` for a concurrency law — or, when `host` is set, any
    /// row at all. Nothing else type-checks.
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
/// M8 started it at `"0.1.0"`; it is `"0.5.0"` as shipped — see the Versions
/// blocks below, the last of which is W4's `law/host`.
pub const PROVER_VERSION: &str = "0.5.0";

impl Store {
    /// The entry answers a borrow of a `CachedObligation`, not an `Evidence`:
    /// the file carries the tier the discharging run *reported* alongside the
    /// evidence, so that a label and an evidence telling different stories is
    /// detectable rather than silently believed. The reader recomputes the tier
    /// from the evidence and discards the entry when the two disagree.
    pub fn obligation(&self, key: DefHash) -> Option<&CachedObligation>;
    pub fn put_obligation(&mut self, key: DefHash, entry: CachedObligation);
    pub fn review_record(&self, def: &Symbol) -> Option<&ReviewRecord>;
    pub fn put_review_record(&mut self, def: Symbol, record: ReviewRecord);
}

pub struct CachedObligation { pub tier: String, pub evidence: CachedEvidence }

/// `specs` holds *sentence* hashes — `HashOutput::spec_texts` for this
/// definition's own clauses, plus `law_texts` for every law naming it directly.
pub struct ReviewRecord { pub def_hash: DefHash, pub specs: Vec<DefHash> }
```

The store persists **evidence**, not a `Discharge`, and that is deliberate:
`CachedEvidence` has no variant for a refutation, a vacuity or a gap, so a cache
that held one would not type-check. `Discharge` is not `Serialize` for the same
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
   refutation. ADR 0007(g) is the full statement.

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

Everything else about the region is ADR 0006's, unchanged: no nesting (E0416),
no escaping `Task` (E0413), stuck is E0414, a divergent replay is E0415.

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
obligation reports — ADR 0006's argument for two separate streams, applied
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
`observe_definitions`** — ADR 0004's rule: a definition exercised by an
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
  in Ply, classified like E0415 and never bisected. This is `--audit-backend` for
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
  BLAKE3 in ADR 0006 instead of a PRNG crate.
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
- **Handler-parametric laws.** A handler is syntax, not a value; ADR 0007
  lists the four things that would change.
- **Bounded integer arithmetic**, runtime contract checking, spec-derived code,
  refinement types, dependent types, and a `--coverage` flag.

---

## Host boundary

`docs/adr/0008-host-effect-boundary.md` has the reasoning and
`docs/adr/0011-the-web-track.md` the decisions; this section is the contract.
**Where it disagrees with any section above, this section wins** — it was
written after them.

### The rule everything else follows from

> Every guarantee Ply has rests on the runtime knowing what a computation can
> do. A host handler is the one place that knowledge can be wrong, so a wrong
> declaration is loud and a missing declaration is fatal — never the reverse.

Three consequences decide everything below. **A host binding may not change what
the front end computes**, or `ply check` answers differently under `--host` and
every cache splits on a flag. **A host binding may not change what a green
result means**, so a run that reached the host is never written to the result
cache. **When the static picture and the dynamic one disagree the dynamic one
wins and says so**, which is `E0427`.

### `ply-eval::host` — the types the machine speaks

A new module in `ply-eval`, carrying no runtime and no dependency on one, so the
machine can dispatch to a host handler without `ply-eval` learning what a socket
is. `ply-host` implements the traits; `ply-eval` only names them.

```rust
/// Which resource labels a registration serves.
pub enum HostResource {
    /// Exactly this label. `Resource::Singleton` for an operation declared
    /// without `[r]`.
    Only(Resource),
    /// Every label the *program* uses with this operation. Resolved against
    /// `CheckOutput` at bind time and never printed as `*`: a driver that claims
    /// every table must list the tables it got.
    Any,
}

/// Whether a handler may serve an effect the program did not declare `nondet`.
/// Consulted at bind time and by `ply hosts`, and **nowhere in inference, in a
/// cache key, or in the evaluator** — see `Determinism propagation` below.
pub enum Determinism { Deterministic, Nondeterministic }

/// Whether replaying this operation changes anything outside the program.
pub enum Linearity {
    /// A send, an insert, a charge. Bumps the machine's `host_ops` counter and
    /// therefore closes multi-shot resumption over it.
    AtMostOnce,
    /// A clock read, a read of an immutable resource. Replay is harmless, so it
    /// costs the linearity rule nothing.
    Repeatable,
}

/// One registration: an `(effect, operation, resource)` triple, a determinism
/// flag, a linearity obligation.
pub struct HostOp {
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: HostResource,
    pub determinism: Determinism,
    pub linearity: Linearity,
    /// May not run on the scheduler's thread; dispatched to `ply-host`'s pool
    /// and answers `Pending` immediately.
    pub blocking: bool,
    /// Whether this operation may be handed a value containing a
    /// `Value::Secret`. Added by W5, where `E0439` and the column are specified.
    pub secrets: bool,
    /// The Rust path, as `ply hosts` prints it. The reviewable identity of a
    /// member of the trusted computing base.
    pub path: &'static str,
}

/// What a handler is called with. `atom` is the *resolved* atom — the concrete
/// resource, never `Any` — so a handler never has to re-derive its own
/// footprint.
pub struct HostRequest<'a> {
    pub atom: EffectAtom,
    pub op: &'a HostOp,
    pub args: &'a [Value],
    pub span: Span,
}

pub enum HostAnswer {
    /// Completed. Returned into the perform site along the ordinary
    /// tail-resumptive path.
    Value(Value),
    /// Did not complete. The performing task leaves the enabled set until the
    /// token resolves, exactly as `sim::Answer::Sleeping` parks one on a
    /// deadline.
    Pending(Pending),
}

/// Opaque to `ply-eval`: minted by a `HostRuntime`, polled by it, and never
/// interpreted here.
pub struct Pending { pub token: u64, pub label: &'static str }

pub trait HostHandler: Send + Sync {
    fn call(&self, rt: &dyn HostRuntime, req: &HostRequest<'_>)
        -> Result<HostAnswer, Diagnostic>;
}

/// The one thing `ply-eval` needs from `ply-host`.
pub trait HostRuntime {
    /// `Ok(None)` when the token has not resolved. The scheduler polls; it never
    /// spins, because `park` is what it calls when nothing is ready.
    fn poll(&self, p: &Pending) -> Result<Option<Value>, Diagnostic>;
    /// Wait until at least one outstanding token resolves. Called only with no
    /// task enabled.
    fn park(&self) -> Result<(), Diagnostic>;
    /// Drive until this token resolves. The only place a Ply computation blocks
    /// a real thread, and reached only outside a scheduler region.
    fn block_on(&self, p: Pending) -> Result<Value, Diagnostic>;
}
```

### Registration — `HostRegistry` and `HostBinding`

```rust
/// Built by one function in `ply-host`, so the trusted computing base is a list
/// read top to bottom in one file. No attribute macro, no link-time registry, no
/// global constructor.
pub struct HostRegistry { .. }
impl HostRegistry {
    pub fn new() -> HostRegistry;
    pub fn register(&mut self, op: HostOp, handler: Arc<dyn HostHandler>);
    /// Registered, listed, and deliberately **not bound by this run**. W5's
    /// `signal` is the case it exists for: a stop flag set once ends every test
    /// after it, so `ply test` binds none of `signal` with or without `--host`.
    /// A `perform` that reaches one is `E0424` with the *withheld* wording —
    /// "this run binds no handler for it", a different sentence from "nothing
    /// registers this", and the only one a reader can act on.
    /// `HostBinding::withholds` is the lookup that tells the two apart.
    pub fn register_withheld(&mut self, op: HostOp, handler: Arc<dyn HostHandler>);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn ops(&self) -> impl Iterator<Item = &HostOp>;
    /// Resolve every registration against the program. Fails loudly; see the
    /// codes below. `Any` resolving to no atom is **not** an error — a driver
    /// linked into a program that never queries is idle, not wrong.
    pub fn bind(self, check: &CheckOutput) -> Result<HostBinding, Vec<Diagnostic>>;
    /// The listing `ply hosts` prints without `--host`: what *would* bind.
    pub fn preview(&self, check: &CheckOutput) -> Result<HostListing, Vec<Diagnostic>>;
}

/// What a run actually has bound. `hermetic()` is the default everywhere.
pub struct HostBinding { .. }
impl HostBinding {
    pub fn hermetic() -> HostBinding;
    /// The registry is retained even when nothing is bound, so `E0424` can name
    /// the handler that *would* have served the operation.
    pub fn hermetic_with(registry: HostRegistry) -> HostBinding;
    pub fn is_hermetic(&self) -> bool;
    /// Every atom this binding serves. The set selection intersects a test's
    /// footprint against, and it is exact rather than an upper bound.
    pub fn footprint(&self) -> &Footprint;
    pub fn serves(&self, atom: &EffectAtom) -> bool;
    /// Whether any atom of a footprint reaches this binding: what `Reason::Host`
    /// and `Isolation::Host` are decided by.
    pub fn reaches(&self, footprint: &Footprint) -> bool;
    /// The resolution the machine performs per `perform` that reached the
    /// boundary. `None` in a hermetic run and for a triple nothing registered.
    ///
    /// **By the triple, never by the atom.** An `EffectAtom` carries no
    /// operation, so `db.get[users]` and `db.peek[users]` are one atom and two
    /// operations; a registry keyed by the atom reports those as `E0422` and
    /// refuses a program that is merely ordinary.
    pub fn resolve(&self, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>)
        -> Option<Bound<'_>>;
    /// What a hermetic `E0424` names: the path that would have served this.
    pub fn would_serve(&self, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>)
        -> Option<&'static str>;
    /// The path of a registration this run **withheld**, for the other `E0424`.
    pub fn withholds(&self, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>)
        -> Option<&'static str>;
    pub fn listing(&self) -> &HostListing;
}

pub struct Bound<'a> {
    pub atom: EffectAtom,
    pub op: &'a HostOp,
    pub handler: &'a Arc<dyn HostHandler>,
}

/// One row per resolved **triple**, ascending by `(effect, op, resource)`. Never
/// one row per registration: an `Any` handler must not hide a resource behind a
/// `*`.
pub struct HostListing { pub rows: Vec<HostRow>, pub handlers: usize }
pub struct HostRow {
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: Resource,
    /// What this triple contributes to a footprint. Not a key.
    pub atom: EffectAtom,
    /// Which registration produced this row.
    pub row: usize,
    pub path: &'static str,
    pub deterministic: bool,
    pub linearity: Linearity,
    pub blocking: bool,
    /// `HostOp::secrets`, carried through so the listing can print it — W5
    /// below, where the field is introduced and the digest widened to cover it.
    pub secrets: bool,
    /// Whether the *declaration* carries `nondet`. Printed so a reviewer can see
    /// the pair that `E0423` checks.
    pub declared_nondet: bool,
}
impl HostListing {
    /// BLAKE3 over the canonical rows, domain-tagged `b"ply.hosts.1"`. Rendered
    /// `b3:` plus the first twelve hex characters. The one line CI pins.
    pub fn digest(&self) -> [u8; 32];
    pub fn digest_short(&self) -> String;
}
```

### The handler signature

Two shapes, and which one is used is a property of the operation rather than of
the run:

- `HostAnswer::Value` — anything that cannot block: a clock read, a byte
  operation, a non-blocking write that took the whole buffer. The machine
  returns it into the perform site along the ordinary tail-resumptive path, so a
  value-shaped host operation costs exactly what a Ply handler clause costs.
- `HostAnswer::Pending` — anything that waits. The performing task leaves the
  enabled set until `HostRuntime::poll` yields a value. Inside a scheduler
  region this is `sim::Answer::Sleeping` with a token instead of a deadline and
  the machine's handling is the same code. Outside one there is no scheduler to
  park on, so the machine calls `HostRuntime::block_on`.

`blocking: true` means the handler's *work* may not run on the scheduler's
thread: it dispatches to `ply-host`'s pool and answers `Pending` immediately.
`call` itself is always entered on the machine's thread, because a `Value` is not
`Send` and nothing in `ply-eval` could hand the work elsewhere.

**Enforced:** a `blocking: true` handler that answers `HostAnswer::Value` did the
work on this thread, and that is `E0428` before the value reaches the perform
site. **Not enforced, and not enforceable:** a handler that blocks while declared
non-blocking stalls every task sharing its thread, with no budget, no watchdog
and — since W1 defers cancellation — no way out. That half is a review
obligation, and `ply hosts` prints the column so there is something to review.

**A handler does not choose the code its failure is reported under.** Every `Err`
from `HostHandler::call` is passed through `ply_eval::host::attribute`, which
appends a note naming the handler path and the operation, and replaces a code in
`RESERVED_CODES` with `E0502`. The reserved set is the codes that decide
classification — `E0505`, `E0503`, `E0415` — plus the ones the boundary and the
machine raise about their own state: `E0413`, `E0414`, `E0416`, `E0421`–`E0428`.
Without the rewrite a handler could report its own failure as a defect
in Ply, which sends the reader to file a bug against the language and suppresses
the diagnosis that would have found the handler. `HostRuntime::poll`, `park` and
`block_on` are **not** rewritten: their failures really are about the reactor's
own invariants.

### Linearity enforcement — implement exactly this

```rust
impl<'a> Machine<'a> {
    /// At-most-once host operations answered in this entry point. Reset by
    /// `reset()` with everything else, so a count never crosses an entry point.
    pub fn host_ops(&self) -> u64;
}

impl Continuation {
    /// `Machine::host_ops` when this continuation was captured.
    pub fn born(&self) -> u64;
    /// Resumptions so far, shared across clones — a `Continuation` is cloned by
    /// `Rc`, and two clones are one continuation.
    pub fn resumes(&self) -> u32;
}
```

A resumption is refused with `E0426` exactly when

```
resumes > 1  &&  machine.host_ops > k.born
```

checked in `handler::resume` and in the `Frame::Resume` path, which are the two
places a continuation is applied. `host_ops` is incremented **only** for
`Linearity::AtMostOnce`, and only after the operation actually ran.

In a hermetic run `host_ops` is zero for the life of the entry point, so **no
existing multi-shot behaviour changes**. That is a required test.

This over-approximates: it refuses a second resumption when an at-most-once host
operation happened anywhere after the capture, including in another task or in
the handler clause rather than inside the continuation. The precise rule needs a
per-resumption liveness scope on the control stack — a new frame kind
interacting with capture, splice, `Next::Leave` and task start — in the one part
of the system where a defect is silent and sends a packet twice. Do not build
it. If the false positive ever bites a real program, that is a contract
amendment with a program attached.

### Determinism propagation — the arrow points the other way

> A handler registered `Determinism::Nondeterministic` requires its effect to be
> declared `nondet` in Ply source. Otherwise `E0423`, at bind time.

**E0412 does not change, inference does not change, and no cache key learns
about a binding.** An effect with a real socket behind it is `nondet` in the
source, which is where E0412 already looks, so a `det` test that reaches it
fails to compile whether or not `--host` was passed. Adding `nondet` to a
declaration is a source edit that moves the hashes it should move.

A `Deterministic` handler is still not cacheable. The flag's entire content is
"this handler may serve an effect the program did not declare `nondet`".

The flag is read in exactly two places: `HostRegistry::bind`, and `HostListing`.

### Hermetic by default

`ply test` and `ply run` bind `HostBinding::hermetic_with(ply_host::registry())`.
`--host` binds for real. There is no environment variable and no config file: a
flag is the only thing that appears in the command a reviewer reads.

**Reaching the boundary unbound is `E0424`, never `E0303`.** E0303 means
inference should have prevented this and did not — a bug-catcher. E0424 means
inference was right and the run was configured hermetically. The two call for
opposite responses. The message names the operation, says `ply test` is
hermetic, and names both remedies plus the handler that would have served it.

**Selection under `--host`.** A test's footprint is an upper bound on what it
performs, so the tests that can reach the host are exactly those whose footprint
intersects `HostBinding::footprint()`. Those get `Reason::Host`: always run,
never read from the cache, never written to it. Every other test is selected
exactly as it would be hermetically. `--host` is **not** `--no-cache`, and a
build that makes it one has made the hermetic default the expensive one.

**The runtime is authoritative.** `Record::Host` is written from what actually
happened. A test that was not predicted to reach the host and reached it anyway
is `E0427`.

**Bisection and hybrids are skipped** for a failing test that reached the host:
`Skipped::Host`. M5 re-runs a failing test many times over mixed definition sets,
and doing that to a test that sends packets sends them that many times. The
suspect set still comes from hashes and is unaffected.

Decided by what the runtime **did** — `Failure::host` is set from `HostUse`, not
from the footprint prediction — and a refusal the handler *returned* counts, so a
handler that acted and then failed is still a host-backed failure.
`precheck(Gate { .. })` orders it after `defect` and **before** `nondet`: nearly
every host-backed test is also `test/nondet`, and the two say different things.
`nondet` says a hybrid's answer would be evidence about nothing; `host` says
asking the question is itself an action on the world.

`diagnose_failures` builds no `BodyHybrid` at all for such a failure, which is
the second lock. The first is that `BodyHybrid::trial` constructs its machine
with no binding and there is no path by which one reaches it — an accident that
saved the packets before `Skipped::Host` existed, and one that a single
`set_host_binding` call to "make bisection work under `--host`" would have
undone.

### World interaction

> **This subsection describes a design, not the code.** None of it shipped. The
> block below is kept because the reasoning in it is still the reasoning, and
> because a claim that was quietly deleted teaches nobody — but `Isolation` has
> two variants and one constructor argument, and `Parallelism` has no `host`
> field. What shipped is stated after the block.

```rust
pub enum Isolation {
    World,
    Shared,
    /// Reaches a bound host handler. Not world-isolated, because a socket cannot
    /// be forked; counted separately so the `isolated: n of m` number stays
    /// honest and `--explain` can say why.
    Host,
}
impl Isolation {
    /// A breaking change, deliberately rather than a second constructor beside
    /// the old one: a caller that kept the one-argument form under `--host`
    /// would report a host-backed test as trivially parallel.
    pub fn of(footprint: &Footprint, hosts: &Footprint) -> Isolation;
    /// `Host` is not `World`. Nothing else changes.
    pub fn is_world(self) -> bool;
}
```

**As shipped**, in `ply_test::schedule`:

```rust
pub enum Isolation { Region, Shared }        // no `Host` variant
impl Isolation {
    pub fn of(footprint: &Footprint) -> Isolation;   // one argument; the binding
                                                     // is not consulted
    pub fn is_isolated(self) -> bool;                // there is no `is_world`
    pub fn as_str(self) -> &'static str;
}
```

`Parallelism` carries `total`, `isolated`, `shared`, `region_contended`,
`scheduled`, `groups` and `shared_groups` — **no `host`**.

The reporting the block above wanted does exist; it is computed one level up. The
`host` count, the per-test `isolation: host` label and the
`host: n of m · not cached` summary line are all built in
`ply-cli`'s test command from `HostView::reaches(check, index)` — the test's
footprint against the binding — and not from a `Parallelism` field or an
`Isolation` variant. So the numbers a reviewer reads are the numbers this
subsection promised, while `ply_test`'s own types know nothing about the host,
and a second consumer of `ply_test` would have to recompute the correction. Both
halves are worth knowing: the printed number is honest, and the library type is
not the place it comes from.

Grouping is otherwise **unchanged**: a host atom is an ordinary contending atom,
so readers-writers over `db.read[users]` still decides what runs beside what,
which is what ADR 0008 asks for and it needs no special case.

### `simulate` and the host are mutually exclusive

A host operation reached from inside a `simulate` region is `E0425`. DPOR
re-runs a test whole per interleaving; a region reaching a socket sends one
packet per interleaving explored and then calls the result a proof over every
interleaving.

The check is one line in the machine: a `perform` that reaches the host binding
with `!self.sims.is_empty()` is `E0425`. It cannot be a static check, because a
`simulate` region and a host-backed perform may be arbitrarily far apart in the
call graph and the row that connects them is discharged in between.

**And the same refusal over the whole test**, because the search re-runs the
*test* and not the region: an operation in the prefix or the suffix around a
region is re-performed exactly as one inside it is, and `innermost_simulation` is
empty in both places. The machine cannot derive that, so the runner states it:

```rust
impl<'a> Machine<'a> {
    /// This entry point is one of several runs of the same test, so reaching
    /// the host boundary is `E0425`. Stated per entry point by the caller
    /// driving the search, like `set_declared_footprint`.
    pub fn set_re_executed(&mut self, re_executed: bool);
}

impl Plan {
    /// Whether running this plan can drive the entry point more than once. An
    /// upper bound: a `Dpor` plan that turns out to have one interleaving is
    /// still refused, because finding out costs the packet.
    pub fn re_executes(&self) -> bool;
}
```

`InterpExecutor::search` sets it from `plan.re_executes() ||
search.measure_reduction` — `--simulation measure-reduction` runs the whole
search a second time unpruned, which doubles whatever the first did. The refusal
precedes the handler call, so the count is zero and not one.

`--simulation once` explores a single interleaving and therefore does not
re-execute, so a host-backed test may run under it. That is the "run it once
without searching" answer, reached by a flag rather than by silently dropping the
search.

Opening a **production region** is exempt: `task.*` reaching the binding performs
nothing outside the program, and the seeded and production schedulers already
exclude each other three ways, so `E0416` is the specific answer there and
refusing first would replace it with a vaguer one.

### The scheduler seam

There is **one** `Scheduler` type, `ply_eval::sched::Scheduler`, gaining:

```rust
pub enum Policy {
    /// Choices from the trail's `sched` stream, time from the virtual clock,
    /// steps recorded. What `simulate` installs.
    Seeded,
    /// The lowest-numbered ready task; `HostRuntime::park` when none is ready;
    /// no steps recorded and no `Exploration` reported. What a host binding that
    /// serves `task` installs.
    Host,
}
impl Scheduler { pub fn policy(&self) -> Policy; }
```

A second `Scheduler` in `ply-host` is exactly the drift M7's design exists to
prevent, and it is not to be written. `ply-host` supplies a `HostRuntime` and
nothing else.

A Ply task cannot move between OS threads — `Value` holds `Rc` and a `Machine`
is single-threaded by construction — so the production scheduler is not one task
per thread. Real threads are confined to the host runtime's reactor and blocking
pool, where no Ply value ever goes.

**The production region opens lazily**, at the first `task.*` perform that
reaches the host binding, rooted at the stack it was performed on, carrying a
`SimId` like any other region and closed by the same `close_regions` path.
Opening one eagerly around every entry point makes every existing `simulate`
nested and `E0416` under `--host`.

**Mutual exclusion, three independent locks**, and no one of them is load-bearing
alone:

1. `task` is `nondet`, so a `det` test performing `task.*` unhandled is `E0412`
   and never runs. Only a `test/nondet` can reach a production scheduler.
2. `simulate` pushes `Delimiter::Sim` and `Stack::find_handler` walks the stack
   innermost-first before the host binding is consulted at all, so a
   `task.spawn` inside a region reaches the seeded scheduler always.
3. Nothing binds without `--host`.

The host binding is consulted **only when `Stack::find_handler` returns `None`**.
It is the handler of last resort, it is not a `Delimiter`, and no `Continuation`
ever contains it — which is what keeps capture, splice and `Next::Leave`
untouched by this milestone.

### The footprint check

When the machine answers a perform from the host binding, it checks the
performed atom against the declared footprint of the entry point being run:

```rust
impl<'a> Machine<'a> {
    pub fn set_host_binding(&mut self, binding: Arc<HostBinding>);
    /// The declared footprint of the entry point about to run. A host answer
    /// whose atom is outside it is `E0427`.
    pub fn set_declared_footprint(&mut self, footprint: Footprint);
    /// What the run actually reached. `None` until a host handler answered.
    pub fn host_use(&self) -> Option<&HostUse>;
}

/// What a run reached across the boundary. Reported, and the authority on
/// whether a green verdict may be cached.
pub struct HostUse { pub atoms: Footprint, pub operations: u64 }
```

`E0427` is `Status::Panicked`, `Failure::defect` is `true`, and it is **not**
bisected — the same class as `E0503` and `E0415`, because the run knows two of
its own answers disagree and nothing in the definition graph decides which was
meant. `ply_test`'s defect predicate therefore reads
`HOST_FOOTPRINT_ESCAPE` alongside `INTERNAL_ERROR` and the two divergence codes.

**Every command that runs an entry point states the claim**, `ply test`
included: `InterpExecutor::arm_footprint_check` is called before each test on
every machine path — `Engines::One`, both halves of `Engines::Audited`, and once
per interleaving in `search`. It is restated per test rather than per worker, because
one `Machine` serves many tests and a claim that outlived its entry point would
judge the next test by the last one's row. A check nothing installs is not a
defence: it was unarmed in `ply test` once, and a `det`, world-isolated test
opened a real TCP listener and was reported green.

This is a `BTreeSet` lookup per host operation, against a syscall. **What it
defends is narrower than "a footprint that under-reports"**, and the difference
decides how much the boundary is trusted for:

- it catches a *program* footprint that under-reports — a `handle` discharges an
  atom, and an atom names no operation, so a clause set covering some but not all
  operations of an atom leaves the rest to reach the binding out of a row that no
  longer mentions it;
- it catches a binding that resolved an atom the program's own footprints never
  enumerated (`err_unenumerated_atom`);
- it **cannot** catch a handler that does more than its registration declared.
  The atom compared is the one the registry computed and a handler has no way to
  report a different one, so `db.read[users]` that also writes is recorded as a
  read, reported as a read and *scheduled* as a read. Since ADR 0008 makes
  footprint conflict grouping the only isolation a host-backed test has, the
  isolation of such a test is exactly as good as its registration's mode and
  resource, and nothing checks either. `ply hosts` plus review is the whole
  defence there.

### `Bytes`

```rust
pub enum Value { /* ... */ Bytes(Arc<[u8]>) }
pub enum Lit   { /* ... */ Bytes(Vec<u8>) }
```

`Arc<[u8]>` mirrors `Value::Str(Arc<str>)` exactly. Not `bytes::Bytes`: what
that crate buys is cheap slicing of a shared buffer, which W3's streaming bodies
want and W1 does not, and it would put a type carrying its own refcount
semantics into the one enum the hygiene rules are written against. Slicing
copies at W1.

Surface syntax: `b"..."`, recognized in the lexer by `b` immediately followed by
`"` with no space. Escapes are the string escapes plus `\xNN`; a source
character above `U+007F` inside a `b"..."` is `UNEXPECTED_TOKEN` telling the
author to write `\xNN`, because "the bytes of this literal" must not depend on
the file's encoding.

`Bytes` is a nullary builtin type constructor in `BUILTIN_TYPES`, and
`Type::bytes()` beside `Type::string()`.

Normalization tag `LIT_BYTES = 44`, length-prefixed raw bytes. A distinct tag
from `LIT_STR` so `b"ab"` and `"ab"` are different definitions — they have
different types and must not share a hash. `BODY_ENCODING` and
`FRONTEND_VERSION` bump.

Builtins, all pure, all monomorphic:

| builtin | type | notes |
| --- | --- | --- |
| `bytes_len` | `(Bytes) -> Int` | |
| `bytes_at` | `(Bytes, Int) -> Int` | `0..=255`; out of range is `RUNTIME_ERROR` |
| `bytes_slice` | `(Bytes, Int, Int) -> Bytes` | half-open; out of range is `RUNTIME_ERROR`, never clamped |
| `bytes_concat` | `(Bytes, Bytes) -> Bytes` | |
| `bytes_concat_all` | `(List<Bytes>) -> Bytes` | one allocation over the whole list; the empty list is `b""` |
| `bytes_of_string` | `(String) -> Bytes` | total, UTF-8 |
| `bytes_is_utf8` | `(Bytes) -> Bool` | the check, so the partial path is avoidable |
| `string_of_bytes` | `(Bytes) -> String` | `RUNTIME_ERROR` naming the byte offset of the first invalid sequence |
| `string_of_bytes_lossy` | `(Bytes) -> String` | total, `U+FFFD` per invalid sequence |

Ply has no `Result` in its prelude, so one partial conversion would be a
landmine. Three builtins are the honest shape of the UTF-8 boundary.

`++` stays `String`-only: overloading it needs type-directed dispatch, which W2
explicitly declines to settle.

`values_equal` compares `Bytes` by content and a `Bytes` is never equal to a
`Str`. `Value::render` prints `b"..."` with non-printable bytes as `\xNN`,
truncating past 32 bytes as a list does. `Value::type_name` is `"Bytes"`.

Pattern matching is exact-literal only, which `PatternKind::Lit` already gives.
Byte-slice patterns are not in W1.

`Bytes` is **quantifiable** in a `forall`: `property.rs` generates length
`0..=32` with uniform bytes, `shrink.rs` has minimal value `b""` and shrinks
length before content. Leaving it ungeneratable would regress M8's guarantee on
contact with a new primitive, which is the class of thing this project audits
for.

W2's derivation treats `Bytes` as a leaf.

### Text

`Bytes` is what a socket speaks; these are what a server does once it has
decoded. All pure, all monomorphic, all in `ply-eval::builtins` beside the
above.

| builtin | type | notes |
| --- | --- | --- |
| `string_len` | `(String) -> Int` | characters |
| `string_slice` | `(String, Int, Int) -> String` | half-open, in **characters**; out of range is `RUNTIME_ERROR`, never clamped |
| `string_split` | `(String, String) -> List<String>` | an empty separator is `RUNTIME_ERROR` |
| `string_trim` | `(String) -> String` | Unicode whitespace, both ends |
| `string_lower` / `string_upper` | `(String) -> String` | full Unicode, locale-independent; the length may change |
| `string_starts_with` / `string_ends_with` | `(String, String) -> Bool` | |
| `string_contains` | `(String, String) -> Bool` | the check, so `string_find` is avoidable |
| `string_find` | `(String, String) -> Int` | the **character** index; absent is `RUNTIME_ERROR` |

**A `String` is indexed in characters and a `Bytes` in bytes**, everywhere and
without exception, so no argument to a `String` builtin can name a position
inside a character and "slicing that would split a character" cannot be
expressed. The split is instead caught where it is actually made: a
`bytes_slice` that cuts a multi-byte sequence produces a `Bytes` that
`string_of_bytes` refuses, naming the offset. Truncating to a boundary, or
substituting `U+FFFD`, would be the silent-wrong-answer shape — and
`string_of_bytes_lossy` is there for a caller that genuinely wants the second.

`string_find` and `string_contains` are the same pair as `string_of_bytes` and
`bytes_is_utf8`, for the same reason: a `-1` sentinel is a value a careless
program hands straight to `string_slice`, and the error then lands far from the
mistake. `len` stays `(List<a>) -> Int` — a String's length is `string_len`
until W2 settles type-directed dispatch.

`string_trim`, `string_lower` and `string_upper` consult the Unicode tables in
`std`, so their answers move if those tables do. A Rust toolchain upgrade is
therefore a `RUNTIME_VERSION` bump, and this is the first place in the language
where that is true.

The prover treats every one of these as opaque; the total ones are in
`TOTAL_BUILTINS`, and the six that refuse an input — `bytes_at`, `bytes_slice`,
`string_slice`, `string_split`, `string_find`, `string_of_bytes` — deliberately
are not.

The list index added by `docs/adr/0027-a-list-index.md` does **not** make it
seven. `list_at` refuses no index — it answers `None` — so it is in
`TOTAL_BUILTINS`. That `bytes_at` raises and `list_at` does not is the one place
two containers in this language are indexed by different conventions, and ADR 0027 is the argument for it — the short version being that ~~a raising list
index would be excluded from `TOTAL_BUILTINS`, which would block a `property`
over any function that peeks, at every peek.~~

**The outcome in that struck clause is right and its mechanism is wrong
(corrected 2026-08-30).** The `TOTAL_BUILTINS` exclusion costs `proved`, and on
a `List` that is currently a cost of nothing, because no `List`-valued term
reaches the decidable fragment. What actually blocks a `property` over an
unguarded raising peek is the *run*: the randomized case is out of range, the
term raises, and the obligation is `unattempted` (`W0604`) rather than
`property`. So the sentence keeps its conclusion — a raising list index would
cost a `property` at every unguarded peek, and `list_at` does not — and drops
`TOTAL_BUILTINS` from the reason. ADR 0027 carries the two-law demonstration.

### `ply hosts`

```
ply hosts [PATH] [--host] [--json] [--digest]
```

One line per resolved triple, ascending by `(effect, op, resource)`. Exit `0`; a
bind failure is exit `2` like any compile error.

Both the operation and the atom are printed. The operation is the row's
identity and says *what* was bound; the atom is what scheduling and isolation
speak in, and deriving it in your head from a mode annotation in another file is
not something a reviewer should have to do.

```
$ ply hosts --host
   4 host handlers · 6 operations · trusted computing base

   OPERATION           ATOM                HANDLER                    DET  LINEAR         BLOCKING
   clock.now           clock.read          ply_host::clock::now       no   repeatable     no
   db.get[orders]      db.read[orders]     ply_host::postgres::read   no   at-most-once   yes
   db.get[users]       db.read[users]      ply_host::postgres::read   no   at-most-once   yes
   db.put[orders]      db.write[orders]    ply_host::postgres::write  no   at-most-once   yes
   net.send[socket]    net.write[socket]   ply_host::tcp::send        no   at-most-once   no
   task.spawn          task.write          ply_host::task::scheduler  no   repeatable     no

   digest: b3:4f19c0a8e2d3
```

Hermetic is a statement, not an empty listing — an empty listing is
indistinguishable from a registry that failed to load:

```
$ ply hosts
   hermetic — no host handler is bound

   6 operations would bind under `--host`; run `ply hosts --host` to list them
```

`--digest` prints `b3:4f19c0a8e2d3` and nothing else, which is what a CI check
pins. `--json` carries `schema_version: 1`, the `binding`, the `digest` and the
rows with every column above.

### `ply test` and `ply run` flags

```
--host        bind real host handlers. Off by default, and the default is the
              point: a suite that silently acquires a live dependency is the
              failure mode this language exists to prevent.
```

`ply test --json` carries `"binding": "hermetic" | "host"` with the listing
digest at the top level, and per test a boolean `"host"` — whether this test's
footprint meets the binding — beside `"isolation"`, which reads `"host"` for such
a test. **Not** a `host` object of `atoms` and `operations`: `HostUse` is held on
the `Machine` and is not carried out to `TestResult`, so the per-test JSON says
*that* a test reaches the host and not *what* it reached.

`ply-test` gains:

```rust
pub enum Reason { /* ... */ Host }   // NOT SHIPPED — see below
pub enum Record { /* ... */ Host }   // a green run that reached the host
pub enum Skipped { /* ... */ Host }  // bisection refused: re-running replays I/O
pub struct TestResult { /* ... */ pub host: Option<HostUse> }  // NOT SHIPPED
```

**Two of those four did not land, and the gap is the read half of ADR 0011**

- `Record::Host` and `Skipped::Host` exist and are the *write* half: a run that
  reached a host handler records `Record::Host`, nothing is stored, and bisection
  refuses. That half is implemented and tested.
- There is no `Reason::Host`. `select` is
  `select(check, hashes, store, plan)` — it takes no binding — so **no test is
  ever selected because it can reach the host**. A host-reaching test runs today
  only because `Reason::Nondet` covers it, and that covers it only because every
  registration in `ply_host::registry` is `Determinism::Nondeterministic`. ADR
  0011 explicitly permits a `Deterministic` registration and says such a handler
  is "still not cacheable"; it is cacheable. A hermetic pass by a test
  whose footprint reaches the binding is read back under `--host` and the test is
  skipped without the host being consulted.
  `crates/ply-test-tests/tests/suite/host_selection_audit.rs` pins that behaviour under a
  `documents_` name so a fix shows up as a diff.
- There is no `TestResult::host`. `Failure::host: bool` is what carries the fact
  to a failing test, read off what the runtime did.

`TestResult::green_but_uncached` is `Record::Exhausted | Record::Unobserved` and
does **not** include `Record::Host`, so a green host-backed test is not announced
as green-but-uncached in the summary. The `host: n of m · not cached` line under
`--explain` is what says it instead, and that line is computed in `ply-cli` from
the binding rather than from the report.

### New diagnostic codes

Added to `ply_span::codes`; existing numbers are unchanged, and the registry pin
test covers all eight.

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0421 | `HOST_OPERATION_UNKNOWN` | a registration names an effect, operation or resource the program does not declare | the host author's |
| E0422 | `HOST_HANDLER_CONFLICT` | two registrations claim one atom | the host author's |
| E0423 | `HOST_DETERMINISM_MISMATCH` | a `nondet` handler for an effect not declared `nondet` | the host author's |
| E0424 | `HERMETIC_BOUNDARY` | an operation reached the boundary with nothing bound | the program's |
| E0425 | `HOST_IN_SIMULATION` | a host operation in a test the search re-runs — inside a `simulate` region, or in the prefix or suffix around one | the program's |
| E0426 | `HOST_CONTINUATION_RESUMED` | a second resumption across an at-most-once host operation | the program's |
| E0427 | `HOST_FOOTPRINT_ESCAPE` | a host answer whose atom is outside the entry point's declared footprint | **Ply's** |
| E0428 | `HOST_BLOCKING_ANSWER` | a `blocking: true` handler answered a value inline instead of a pending token | the host author's |

E0421–E0423 are raised by `HostRegistry::bind` before anything runs, so they are
start-up failures with no span in user source; they carry the effect
declaration's span as a secondary label when there is one to point at.

E0424, E0425, E0426 and E0428 join `E0501`/`E0502`'s row: `Failure::defect` is
`false` and they are attributed like any other failure — except that bisection is
skipped for a failure the run reached the host to produce, which E0426 and E0428
always are and E0424 and E0425 never are, because both of those refuse before the
handler is called.

E0427 joins `E0503`/`E0415`'s row: Ply's fault, `Status::Panicked`,
`Skipped::Panicked`, no bisection.

None of these eight may be raised by a host handler: they are in
`RESERVED_CODES`, and a handler that returns one has its code rewritten to
`E0502` with a note saying what it claimed.

### Versions

`RUNTIME_VERSION` bumps to `0.6.0`: `Value` gains a variant and the evaluator
gains a dispatch path, and a cached `Pass` is a claim about what the evaluator
did. `FRONTEND_VERSION` bumps to `0.8.0` and `BODY_ENCODING` bumps, for
`Lit::Bytes` in the AST and `LIT_BYTES` in the normalizer. `PROVER_VERSION`
bumps for the `Bytes` generator, which changes what a `property` discharge
sampled.

### Workspace

One new crate, `ply-host`, and one new dependency:

```toml
ply-host = { path = "crates/ply-host" }
tokio = { version = "1.53.1", features = ["rt", "net", "time", "sync"] }
```

`rt-multi-thread` is deliberately absent: a work-stealing runtime is unusable
here because nothing it would steal is `Send`. Tokio earns its place as the
reactor and the timer wheel.

Rejected, each with a reason: **mio** — tokio wraps it, and taking both means two
reactors. **socket2** — buys socket options W1 does not set. **bytes** — see the
`Bytes` section. **httparse** — W1's endpoint returns a fixed response and reads
to the header terminator; a parser in the trusted computing base can wait until
W3 needs one.

The blocking pool is `std::thread` owned by `ply-host` rather than
`tokio::task::spawn_blocking`, so its size is a declared, reviewable number
instead of tokio's default of 512 — a number nobody chose, which decides how many
real connections a runaway test can open.

`ply-eval` gains **no** dependency. `ply-host` depends on `ply-eval`,
`ply-core`, `ply-span` and tokio; `ply-test` and `ply-cli` depend on `ply-host`.

### Required tests

These are the ones whose absence would let W1 ship broken rather than merely
incomplete.

1. A registration for an effect the program does not declare is `E0421`, and the
   message names the nearest declared operation.
2. A registration for a declared effect but an undeclared operation is `E0421`.
3. A registration for `Only(users)` where the program never says `[users]` is
   `E0421`; an `Any` that resolves to nothing is **not** an error.
4. Two registrations claiming one **triple** are `E0422`, naming both paths;
   two operations of one effect sharing one **atom** — `db.get[users]` and
   `db.peek[users]` — are two rows and not a conflict.
5. A `Nondeterministic` handler for an effect without `nondet` is `E0423`,
   pointing at the declaration.
6. `Any` expands to exactly the resource labels the program uses, and `ply hosts`
   prints one row per expanded atom and never a `*`.
7. Binding an empty registry is a legal hermetic binding.
8. **A binding changes no definition hash, no inferred row, and no E0412
   verdict.** `ply check` output is byte-identical with and without `--host`.
9. A `det` test performing a host-backed `nondet` operation is `E0412` at compile
   time, with `--host` and without it, and does not run either way.
10. A hermetic run reaching the boundary is `E0424` and names the handler that
    would have served it.
11. `--host` runs the test and it passes; the same test is `E0424` without it.
12. A host operation inside a `simulate` region is `E0425`, whether the region is
    lexically enclosing or reached through a call.
13. `task.spawn` inside a `simulate` region reaches the seeded scheduler even
    when a host `task` handler is bound — one interleaving per seed, replayable.
14. `--host` on a corpus with no host atoms selects and caches **exactly** as
    the hermetic run does: same selection, same cache writes.
15. A test whose footprint intersects the binding's is `Reason::Host`, runs, and
    writes no cache entry; a second `--host` run runs it again.
16. A `--host` pass is not readable by a hermetic run and vice versa.
17. **In a hermetic run `host_ops` is zero and every existing multi-shot test
    behaves identically.** No M6 test changes.
18. A continuation resumed twice across an `AtMostOnce` host operation is
    `E0426`, and the real operation ran exactly once.
19. The same program with the operation registered `Repeatable` resumes twice
    and performs it twice, deliberately.
20. A continuation captured *after* the last host operation resumes twice.
21. A host handler answering an atom outside the entry point's declared
    footprint is `E0427`, `Status::Panicked`, and is not bisected.
22. A failing host-backed test reports `Skipped::Host` and still has its static
    suspect set.
23. `Isolation::of` returns `Host` for a footprint meeting the binding, `Shared`
    for the same footprint under a hermetic binding, and the `isolated` count
    excludes it.
24. `--explain` reports a host-backed test as `host` and the trivially-parallel
    count does not include it.
25. `ply hosts --digest` is stable across runs and changes when any column of any
    row changes — including `linearity` and `blocking` alone.
26. `ply hosts` without `--host` says `hermetic` and still reports how many atoms
    would bind.
27. `b"ab"` and `"ab"` have different definition hashes.
28. A `b"..."` literal with a non-ASCII source character is `UNEXPECTED_TOKEN`
    telling the author to write `\xNN`.
29. `bytes_of_string` after `string_of_bytes` is identity on valid UTF-8, and
    `string_of_bytes` on an invalid sequence is `RUNTIME_ERROR` naming the byte
    offset.
30. `string_of_bytes_lossy` is total over every byte string the generator
    produces.
31. `bytes_slice` out of range is `RUNTIME_ERROR` rather than a clamp.
32. A `forall (b: Bytes)` law is discharged rather than `E0418`, and its
    counterexample shrinks toward `b""`.
33. A `Bytes` pattern matches exactly and a `Bytes` never equals a `Str`.
34. `--audit-backend` on a corpus containing `Bytes` reports no `E0503`.
35. A production-scheduled run and a simulated one of the same program produce
    the same value, and only the simulated one reports an `Exploration`.
36. A `simulate` entered while a `Policy::Host` region is live is `E0416`.
37. `HostAnswer::Pending` outside any scheduler region blocks and returns the
    value; inside one it parks the task and another task runs meanwhile.
38. `Store::open` at 10,000 definitions stays under 5ms with `Bytes` in the
    encoding.

Plus one `tests/fixtures/` entry per new code, as every milestone owes.

### Not in W1

- **Cancellation.** A `Pending` has no cancel path, so a task blocked on a host
  operation blocks until it completes or the run ends. **This is the largest gap
  in the milestone** and W3's timeouts need it closed.
- **Backpressure and partial writes.** A socket write that takes part of a buffer
  is the TCP handler's problem, not the boundary's.
- **A host handler written in Ply.** The boundary is where Ply stops.
- **Detecting a handler that does more than it answered.** `E0427` catches a
  handler answering outside its registration. Nothing catches a handler that
  opens a file behind Ply's back, and nothing in this design can.
- **Making a host run replayable.** There is no recording layer, so a host-backed
  failure has no seed and no repro command.
- **Byte-slice patterns**, a `Map`, JSON, routing, TLS, a database.
- **Multi-shot across the host by any mechanism.** The restriction is on the
  boundary, not on the feature, and there is no flag to relax it.

---

## Payloads

`docs/adr/0011-the-web-track.md` has the reasoning; this section is the contract.
**Where it disagrees with any section above, this section wins** — it was
written after them.

### The rule everything else follows from

Every new thing W2 adds is a **value or a definition like any other**. A stdlib
definition hashes like a project one, a derived definition hashes like a
hand-written one, and a `Map` is a value whose canonical form is a function of
its contents. Nothing here gets a private channel into a cache key, a hash, or
an iteration order.

### The stdlib — `ply-std`, a new crate

`import std.json`. `std` is a **reserved first segment**: a project file whose
path would derive a module name of `std` or `std.*` is `E0113
RESERVED_MODULE_NAME` against the file. Reserving it removes any precedence
question between the project and the stdlib.

Sources live in `crates/ply-std/ply/*.ply` and are **embedded at compile time by
an explicit `include_str!` table** in one file — not a directory scan, and not a
path resolved at run time, which would make a program's hashes depend on the
installation layout. There is no `--std-path`.

```rust
/// The module a name denotes, and the source that ships for it.
pub fn source(module: &ModuleName) -> Option<&'static str>;
pub fn modules() -> impl Iterator<Item = ModuleName>;
/// The pseudo-path a module's fingerprint is keyed by: `<std>/json.ply`.
pub fn pseudo_path(module: &ModuleName) -> PathBuf;
/// BLAKE3 over the canonical list of `(module name, hash of source bytes)`.
/// Raw bytes rather than a `ContentHash`, so this crate needs no `ply-store`.
pub fn digest() -> [u8; 32];
pub fn is_std(module: &ModuleName) -> bool;   // first segment is `std`
```

Loading is **demand-driven**: after the project's files are parsed, the loader
walks the import graph and pulls in an `import std.x` no project module
satisfies, transitively. A missing `std.x` is `E0106 UNKNOWN_MODULE` listing
what exists. A program importing nothing from `std` loads nothing and has
byte-identical hashes to what it has today.

- A `std` module may import only `std.*`; anything else is `E0505
  INTERNAL_ERROR`, because the user cannot have caused it or fix it.
- `Loaded::entry_points` excludes `std` modules.
- **A `std` module's tests are not selected by a project run** and write nothing
  to a project's cache. `ply test --std` includes them; the compiler's own suite
  loads the embedded set as one program and checks and runs it.
- A stdlib definition **normalizes exactly as any other**: no `std` marker and no
  stdlib version enters a hash. Copying a `std` source into a project therefore
  produces identical `DefHash`es sharing its cache entries.
- **The stdlib digest is in no cache key.** A digest in the key would invalidate
  a project on an edit to a `std` module it never imports. The store records the
  digest it was last written under and warns `W0605 STDLIB_CHANGED` once when it
  differs, naming both digests and the count of reached definitions whose hash
  moved — often zero, and the warning must say so.
- Gate 1 needs no new mechanism: an embedded module's
  `SourceFingerprint::content_hash` is over the **embedded source bytes** and its
  store key is the pseudo-path. Gate 2 is unchanged. `prune`'s `keep` gains the
  pseudo-paths the run loaded.

`crates/ply-host/ply/net.ply` moves to `crates/ply-std/ply/net.ply` as module
`std.net`; its demo functions move to `examples/`. The effect's program-wide name
becomes `std.net.net`, which `ply_host::tcp`'s registration and `ply hosts` must
use.

`ply std [--json] [--digest]` lists the modules with definition counts and the
digest; `--digest` prints `b3:` plus twelve hex characters and nothing else,
exactly as `ply hosts --digest` does.

### `Map`

```rust
pub enum Value { /* ... */ Map(RedBlackTreeMap<Value, Value>) }
```

`rpds::RedBlackTreeMap`, which `World` already uses, so **no new dependency**,
and under the same `RcK` shared-pointer kind, so a `Value` stays thread-confined.
Persistent: `map_insert` is O(log n) and a clone is a refcount bump.

This requires `impl Ord for Value`, hand-written and **structural, total and
deterministic**: variants by a pinned discriminant first; `Decimal` by numeric
value; `Float` by `f64::total_cmp`; `List` lexicographically; `Record`
field-name-ascending; `Ctor` by variant name then fields; and `Closure`, `Cell`,
`Task`, `Continuation` by discriminant alone — never a panic and never a
pointer, because one is banned on a reachable path and the other is not
deterministic. Those cases are unreachable from a well-typed program.

`values_equal` stays the language's equality and is **not** rewritten in terms
of `cmp`; the two are checked against each other, which is what catches a
divergence rather than hiding it. Rust's `==` on a `Value` therefore agrees with
the language's everywhere except `Float` NaN, where `cmp` is `Equal` and
`values_equal` is false — so a call site that means the language's equality must
say so.

**`map_keys` is ascending by that order, always.** Not insertion order, not hash
order, not unspecified. Content addressing, the result cache, seeded replay and
`--audit-backend` all assume a value has one canonical form, and every failure a
hash-ordered map would produce is a green result or a red result over correct
code.

> **Corrected (regression audit, 2026-08-21).** Ascending order is necessary and
> was not sufficient: `Value::cmp` is coarser than rendering at `Decimal`, so a
> map held whichever of `1.50m` / `1.5m` was inserted last and `map_keys` was a
> function of insertion history anyway. A key is now reduced to the canonical
> member of its class on the way in (`ply_eval::value::canonical_key`, reached
> from `ply_eval::value::insert_key`, which is the one site every `Map` insert
> passes through). `docs/adr/0019-value-representation.md` §7 is the write-up.

A key type must be **ordered**, which is exactly `derivable(ord, k)` — one
predicate, shared with derivation. Ordered: `Int`, `Bool`, `String`, `Bytes`,
`Unit`, `Decimal`, and structurally `List`, records, ADTs and `Map` over ordered
types. Not ordered: `Float` (NaN makes `<` non-total), function types, `Cell`,
`Task`. `Map<Float, v>` is `E0206 NOT_DERIVABLE`. For a type parameter the
constraint is required at the **signature** — `where derivable(ord, k)` — and
omitting it is `E0206` naming the clause to add.

All pure except `map_fold`; every `k` carries `derivable(ord, k)`.

| builtin | type |
| --- | --- |
| `map_new` | `() -> Map<k, v>` |
| `map_insert` | `(Map<k, v>, k, v) -> Map<k, v>` |
| `map_update` | `(Map<k, v>, k, (v) -> v / e) -> Map<k, v> / e` |
| `map_get` | `(Map<k, v>, k) -> Option<v>` |
| `map_contains` | `(Map<k, v>, k) -> Bool` |
| `map_remove` | `(Map<k, v>, k) -> Map<k, v>` |
| `map_len` | `(Map<k, v>) -> Int` |
| `map_keys` | `(Map<k, v>) -> List<k>` |
| `map_values` | `(Map<k, v>) -> List<v>` |
| `map_entries` | `(Map<k, v>) -> List<{key: k, value: v}>` |
| `map_of_entries` | `(List<{key: k, value: v}>) -> Map<k, v>` |
| `map_merge` | `(Map<k, v>, Map<k, v>) -> Map<k, v>` |
| `map_fold` | `(Map<k, v>, b, (b, k, v) -> b / e) -> b / e` |

### `List`

`Value::List` is a radix trie of 32-wide nodes with its newest leaf held apart
(`ply-eval::list`, ADR 0034), so a position is a bounds-checked load for a list
no longer than a leaf and a walk of one node per level past that, then a clone
of the element. A push down a uniquely held path writes in place; a push onto
a shared list copies one leaf and one node per level; a `[x, ..rest]` pattern
is an offset, not a copy. All pure, all total.

| builtin | type |
| --- | --- |
| `len` | `(List<a>) -> Int` |
| `push` | `(List<a>, a) -> List<a>` |
| `list_at` | `(List<a>, Int) -> Option<a>` |
| `map` | `(List<a>, (a) -> b / e) -> List<b> / e` |
| `filter` | `(List<a>, (a) -> Bool / e) -> List<a> / e` |
| `fold` | `(List<a>, b, (b, a) -> b / e) -> b / e` |
| `range` | `(Int, Int) -> List<Int>` |

`list_at` answers `None` for a negative index and for one at or past the end. It
does not clamp and it does not count from the end, so `list_at(xs, -1)` is
`None` rather than the last element — which is `list_at(xs, len(xs) - 1)`. There
is no `head` and no `last`: one primitive spells both, and `len` on an
`Arc<Vec>` is O(1) so the second is not a traversal. ADR 0027.

**There is deliberately no `list_at_or(xs, i, default)`**, and the reason is a
number rather than a taste. It was designed beside `list_at` under a gate fixed
before the measurement — it had to be 1.5× faster per peek to earn a second
name — and it measured **1.26×** at 14,742 elements. A peek in this evaluator is
~1.7 µs and is almost entirely interpreter dispatch; the `Some` allocation and
the `match` that `_or` removes are 0.34 µs of it. The same measurement prices
`map_get` at ~1.7 µs, i.e. **within about a tenth of `list_at`** — so a
`Map<Int, v>` used as an array is not the cost it looks like either. (2% apart
at 14,742 elements, which is inside that rig's resolution; 1.10× at 128,000,
where it resolves. ADR 0027)

The list side's early-exiting driver, which takes no list at all:

| builtin | type |
| --- | --- |
| `iterate` | `(a, Int, (a) -> Iter<a, b> / e) -> b / e` |

`iterate(seed, budget, step)` applies `step` to the seed until it answers
`Stop(r)`, and answers that `r`. `Continue(s)` is the next seed.
`Iter<s, r> = Continue(s) | Stop(r)` is a **prelude** ADT, so `Iter` is reserved
and a project's own `type Iter` is `E0105`; the constructor names are not
reserved and a module may shadow them — **and a module that shadows one cannot
call `iterate` at all**, because its own `Continue` or `Stop` is what the name
then means and it will not unify with `Iter<s, r>`; there is no qualified
spelling that reaches past the shadow, so the remedy is to rename the module's
own constructor. (A *type* named `Stop`, as `std.signal` has, is a different
namespace and costs nothing.) The budget is the most rounds the loop
may take and is spent one per application: exhausting it is `RUNTIME_ERROR`
saying so and **not** phrased as a recursion limit, because nothing nested; a
budget below `1` is `RUNTIME_ERROR` refused before the first round. The callback
is last for the reason every callback builtin's is — `region_kind`'s analysis
reads it as `args.last()`.

`map_insert` replaces an equal key's entry, **key and value both** — the last
write wins, which is visible only for `Decimal`, where `1.5m` inserted over
`1.50m` leaves the key `1.5m`. `map_of_entries` and `map_merge` let the later
side win, by the same rule. `map_remove`
of an absent key is a no-op. `values_equal` compares length then entries in key
order; `render` prints `{k: v, ...}` in key order, truncating past 32;
`type_name` is `"Map"`. Quantifiable in a `forall`: 0..=8 entries, shrinking
entries out before values, minimal `map_new()`. Opaque to the prover.

**No map literal syntax in W2.**

### Derivation — AST landed in `ply-syntax::ast`

```rust
pub enum Deriver { Json, Eq, Ord }
impl Deriver {
    pub const ALL: &'static [Deriver];
    pub fn from_name(name: &str) -> Option<Deriver>;
    pub fn as_str(self) -> &'static str;
    pub fn dictionary(self) -> &'static str;   // "JsonCodec" | "EqDict" | "OrdDict"
    pub fn tag(self) -> u8;                    // pinned: 1, 2, 3
    pub fn from_tag(tag: u8) -> Option<Deriver>;
}

pub struct DeriveDef { pub deriver: Deriver, pub deriver_span: Span,
                       pub target: Ident, pub span: Span }
pub struct Constraint { pub deriver: Deriver, pub deriver_span: Span,
                        pub param: Ident, pub span: Span }
pub struct Derived { pub deriver: Deriver, pub target: Symbol }

pub enum Item { /* ... */ Derive(Box<DeriveDef>) }
pub struct FnDef { /* ... */ pub constraints: Vec<Constraint>,
                             pub derived: Option<Derived> }
```

`Item::Derive` declares no name and generates no node: `Item::name`,
`Item::visibility`, `resolve::declarations_of`, `graph`, `interp`, `machine` and
the driver all skip it, and that is correct rather than a stub, because expansion
has already appended the definitions it stands for.

Grammar:

```
item       := "pub"? (fnDef | typeDef | effectDef) | testDef | lawDef | deriveDef
deriveDef  := "derive" IDENT "for" IDENT
fnDef      := "fn" IDENT generics? "(" params ")" ("->" type)? ("/" row)?
              whereClause? specClause* ("=" expr | block)
whereClause:= "where" constraint ("," constraint)*
constraint := "derivable" "(" IDENT "," IDENT ")"
```

`where` sits between the effect row and any `requires`. Derivers are `json`,
`eq`, `ord`; anything else is `E0207 UNKNOWN_DERIVER`. **`row` waits for W4**,
with the `Row` type it is a codec over.

```ply
type JsonCodec<a> = { encode: (a) -> Json, decode: (Json) -> Result<a, DecodeError> }
type EqDict<a>    = { eq: (a, a) -> Bool }
type OrdDict<a>   = { compare: (a, a) -> Ordering }
```

- **Ply has no top-level value definitions**, so a derivation generates a
  *function*: `fn order_json() -> JsonCodec<Order>`, and
  `fn pair_json<x, y>(x: JsonCodec<x>, y: JsonCodec<y>) -> JsonCodec<Pair<x, y>>`
  for a parameterized type, with the implied constraints.
- **Naming**: `snake_case(TypeName) ++ "_" ++ deriver`. Snake case inserts `_`
  before an uppercase following a lowercase or digit, and before the last
  uppercase of a run followed by a lowercase. A collision is `E0105
  DUPLICATE_DEFINITION` pointing at both `derive` lines.
- `derive` carries no `pub`; a generated definition takes the **target type's**
  visibility.
- **Orphan rule**: a `derive` may only name a type its own module declares, else
  `E0208 ORPHAN_DERIVE`. Two `derive json` for one type are `E0105`.
- **Structural and total.** Leaves: `Int`, `Bool`, `String`, `Bytes`, `Unit`,
  `Float`, `Decimal`. Structural: records, `List`, `Map`, ADTs, `Option`,
  `Result`, aliases through their body. Refused: any function type, `Cell`,
  `Task`, a continuation — `E0206 NOT_DERIVABLE` **naming the field**. `ord`
  additionally refuses `Float`.
- **`json` additionally refuses an `Option` whose payload also encodes as
  `null`** — `Option<Unit>`, `Option<Option<a>>`, and either through an alias.
  `option_json` writes `None` as `null` and `Some(x)` as `x`, so those two are
  one document: `Some(())` decodes back as `None`, and as a `Map` key a
  two-entry map decodes to one. Tagging the encoding instead would change the
  wire format of every optional field, which is the case a payload actually has.
  The refusal is asked twice, of one predicate: `ply_derive::walk` names the
  field before a body is generated, and `ply_core`'s walk over the *solved* type
  catches the spelling the syntactic one cannot see, because an alias is
  expanded by then. The residual: an `Option<p>` under a type parameter `p`
  instantiated at `Unit` is not refused — the constraint that would state it is
  `derivable(json, p)` on `option_json`, which is about `p` rather than about
  `Option<p>`, and the language has no way to say the second.
- **`eq` and `ord` name nothing a module can claim.** `eq` emits the `==`
  operator; `ord` emits `compare_values`, a **reserved** builtin — redefining it
  is `E0105`. A bare `compare` would not do: ADR 0001 says a module's own items
  shadow the prelude, so `fn compare` in the deriving module would silently
  become the order of every dictionary derived in it while `derivable(ord, T)`
  still called the type ordered — the second order §2 rests on not existing.
  `compare` remains the same operation under a name a module may shadow.
- **A `Map` key's wire form follows its type, not its spelling.**
  `Map<String, v>` is a JSON object and every other `Map` is an array of pairs,
  and the key is resolved through **this module's own parameterless aliases**
  first: an alias is transparent to the checker, so `type Key = String` would
  otherwise give one type two codecs that substitute for each other at every
  call site and disagree about the protocol. Only this module's aliases, because
  expansion must be a function of the file — gate 1 keys on raw file content, so
  a cross-module alias entering the decision would leave a stale codec behind
  when that module changed. A cross-module alias to `String` therefore still
  gets the pair form.
- **Composes through named types, never by inlining.** A `users::User` field
  generates a call to `users::user_json()`, so `order_json`'s hash depends on
  `user_json`'s. A generated reference that fails to resolve is reported as
  `E0206` against the `derive` item, not as a bare `E0101`.
- **Expansion runs after parse, before resolution**, purely syntactically.
  Generated `FnDef`s are **appended to `Module::items`** after every source item,
  in `derive` order — one list, because a second is a thing a walker can forget —
  which leaves every `test` and `law` index untouched. Each carries
  `derived: Some(..)`, which is **erased by normalization**.
- **A generated definition that fails to typecheck is `E0505 INTERNAL_ERROR`**,
  Ply's fault: derivation is total, the user did not write the body, and there is
  nothing to attribute it to.
- `DefEntry::span` is the `derive` item's `FileSpan`, and so is every span inside
  the generated body.
- **Any change to a deriver bumps `FRONTEND_VERSION`**, added to the list beside
  normalization, inference, `Scheme`, `Footprint` and the prelude's signatures —
  gate 1 keys on raw file content and would otherwise reuse a stale generated
  definition. A golden pin test renders a fixture type's generated form and says
  to bump.

**Constraints are checked at the signature.** At a call site instantiating `a`
with a concrete `T`, `derivable(D, T)` is checked and failure is `E0206` **at the
call site** with the `where` clause as a secondary label; inside the body the
constraint is assumed. A constraint on an unbound parameter is `E0102`.

**Constraints are kept by normalization** — landed. `tag::CONSTRAINT = 95`, the
parameter's de Bruijn level and the deriver's pinned tag, **sorted and
deduplicated**; a constraint naming a parameter the signature does not bind
contributes nothing. So adding a constraint moves the hash, reordering two does
not, and renaming the parameter does not. The reason is soundness, not taste:
gate 2 rechecks only a definition whose hash moved, so an erased constraint would
leave a caller accepted against a signature that no longer admits it.

### Numeric types

`Float` is IEEE-754 binary64. `Decimal` is `rust_decimal::Decimal` — sign, a
96-bit mantissa, scale `0..=28` — chosen over `bigdecimal` because it is
**bounded**, which is what a value entering a hash and a cache key needs. `ryu`
and `itoa` are rejected: Rust's own formatting already round-trips.

```rust
pub enum Lit   { /* ... */ Float(f64), Decimal { mantissa: i128, scale: u32 } }
pub enum Value { /* ... */ Float(f64), Decimal(rust_decimal::Decimal) }
```

`Lit::Decimal` carries mantissa and scale so `ply-syntax` and `ply-hash` take no
numeric dependency. Surface: `1.5` is a `Float`, `1.50m` a `Decimal`. A literal
past the mantissa or scale limit is `E0001 UNEXPECTED_TOKEN` naming it.

Normalization `LIT_FLOAT = 45` (the IEEE **bit pattern**) and `LIT_DECIMAL = 46`
(mantissa then scale). `1`, `1.0` and `1m` are three definitions; `0.0` and
`-0.0` are two. **A literal's scale is preserved**, so `1.50m` and `1.5m` are
equal in value, differently hashed, and one map key whose retained form is the
first inserted.

Float: IEEE semantics unmodified — `1.0/0.0` is `Infinity`, `0.0/0.0` is `NaN`,
neither is an error; `NaN != NaN`; rendering is shortest-round-trip **always with
a `.0` or an exponent**; **encoding a non-finite `Float` as JSON is a
`RUNTIME_ERROR`** naming the value; not an ordered key type and not derivable for
`ord`.

Decimal: `+` and `-` are exact or `RUNTIME_ERROR` on mantissa overflow — never a
wrap and never a silent rounding; `*` is exact to scale 28 and otherwise rounds
**half-to-even** there; `%` is exact and is therefore allowed where `/` is not,
because the remainder of a decimal division is a decimal even when the quotient
is not, and a zero divisor is `RUNTIME_ERROR`.
**`/` is refused with `E0209 DECIMAL_DIVISION`**, naming `decimal_div`, because
an operator would have to round and a rounding nobody wrote down is the defect
the type exists to prevent.

| builtin | type |
| --- | --- |
| `decimal_div` | `(Decimal, Decimal, Int, Rounding) -> Decimal` |
| `decimal_round` | `(Decimal, Int, Rounding) -> Decimal` |
| `decimal_of_int` | `(Int) -> Decimal` |
| `int_of_decimal` | `(Decimal, Rounding) -> Option<Int>` |
| `float_of_decimal` | `(Decimal) -> Float` |
| `decimal_of_float` | `(Float) -> Option<Decimal>` |
| `decimal_of_string` | `(String) -> Option<Decimal>` |
| `decimal_to_string` | `(Decimal) -> String` |

A scale outside `0..=28` is `RUNTIME_ERROR`. `decimal_of_float` yields the
**shortest decimal that round-trips the float**.

**The prelude gains four ADTs**, because a builtin whose type names a type the
user must import is incoherent:

```ply
type Option<a>    = None | Some(a)
type Result<a, e> = Ok(a) | Err(e)
type Ordering     = Less | Equal | Greater
type Rounding     = HalfEven | HalfUp | Down | Up | Ceiling | Floor
```

> **Still four here, five in the prelude (noted 2026-08-27, ADR 0022).** This
> heading and block are W2's contract and are left as written: W2 did add
> exactly these four. `ply_core::prelude::ADTS` now holds a fifth,
> `Iter<s, r> = Continue(s) | Stop(r)`, added by ADR 0022 for `iterate` and
> specified in §"the list side's early-exiting driver" above. Recorded here
> because this block is the only place in this file that reads as a complete
> list of the prelude's ADTs, and it no longer is.

They join `BUILTIN_TYPES`, so a user `type Option<a>` becomes `E0105`. **This is
a breaking change**: `examples/ledger.ply` and fixtures in `ply-syntax` and
`ply-cli` declare their own `Option` and must be migrated as part of this work,
not after it.

**What this may not do to `proved`.** The linear-arithmetic fragment is over
`Int` and does not extend.

- **`Float` is excluded from `proved` entirely** — `==` is not reflexive, so
  congruence closure over it is unsound. Any obligation whose term graph mentions
  a `Float`-typed term is `property`, never `proved`, and this is a **structural
  refusal**: lowering returns unsupported, so the certificate cannot be built.
  "Mentions" is asked of the type's **declaration**, not of its written form:
  `type Money = Cents(Float)` is `Type::Con("Money", [])` and nothing in it says
  `Float`, while `Cents(NaN) == Cents(NaN)` is still false.
  `ply_prove::prove::Context::reaches_float` is the least fixed point over
  `CheckOutput::ctors`, so a chain, a recursive declaration and a container over
  one all settle; it over-approximates a parameterised declaration, which costs
  completeness and never soundness. `ply_core::derivable` answers the same
  question over the same declarations — that is what refuses `Map<Money, v>` —
  and the two may not disagree.
- **`Decimal` may appear only as an uninterpreted term**: congruence and
  structural equality are sound over it, arithmetic and ordering are not.
- Generators: `Float` draws finite values **and the specials** — `0.0`, `-0.0`,
  `±Infinity`, `NaN`, `±MIN`, `±MAX` — shrinking toward `0.0`; `Decimal` draws
  scale `0..=6` plus `MIN`/`MAX`, shrinking toward `0m` and scale 0.
- `PROVER_VERSION` bumps.

**JSON.** `type Json = Null | Bool(Bool) | Number(Decimal) | Str(String) |
Array(List<Json>) | Object(Map<String, Json>)`. `Number` holds a `Decimal` and
**never an `f64`** — routing numbers through binary64 loses the hundredth of a
cent that `Decimal` exists for — and `Object` is a `Map`, so key order is
ascending and re-encoding is stable. **The limit**: a JSON number outside
`Decimal`'s range or past 28 significant digits is a decode error naming the byte
offset, and the document is rejected **whole** even where the codec would never
have read that field.

**`to_bytes` is bounded by the same `max_depth` `parse` is**, and raises past it
naming the bound. The symmetry is the contract: an encoder that wrote what its
own parser refuses is a codec whose encode is total where its decode is not, so
a service persists or transmits a payload it can never read back and the failure
surfaces at the consumer. An ADT level costs two JSON levels — an object wrapping
a `values` array — so a derived codec over a recursive type reaches the bound at
about half of `max_depth`.

**`float_of_decimal` is the nearest `f64`**, computed through the decimal's own
digits rather than through `Decimal::to_f64`, which divides a mantissa by a power
of ten in binary and is off by an ulp for a long scale. That is what makes
`float_of_decimal(decimal_of_float(f)) == f` for every `f` that has a `Decimal`
at all, and therefore what makes a `Float` field's derived codec lossless on its
whole domain. The domain is `decimal_of_float(f) != None`, and a law that guards
on it is discharged as `property` — the finite-only mode a law can ask for. An
*unguarded* round-trip law over a `Float`-bearing type is still `unattempted`,
because the generator draws `NaN` deliberately and JSON has no non-finite
literal; the gap names the value.

### Byte-oriented builtins

W1 measured 5.41 microseconds per byte of request head: five O(n) folds, each
boxing a `Value::Int` per byte with no early exit. These fix the pass count.

All pure except `bytes_position`. An out-of-range position is `RUNTIME_ERROR`,
never clamped.

| builtin | type | notes |
| --- | --- | --- |
| `bytes_index_of` | `(Bytes, Bytes) -> Option<Int>` | an empty needle is `Some(0)` |
| `bytes_index_of_from` | `(Bytes, Bytes, Int) -> Option<Int>` | absolute index; `from` in `0..=len` |
| `bytes_index_of_byte` | `(Bytes, Int) -> Option<Int>` | the byte is `0..=255` |
| `bytes_starts_with` | `(Bytes, Bytes) -> Bool` | |
| `bytes_ends_with` | `(Bytes, Bytes) -> Bool` | |
| `bytes_split` | `(Bytes, Bytes) -> List<Bytes>` | an empty separator is `RUNTIME_ERROR` |
| `bytes_scan` | `(Bytes, Int, Bytes, Int) -> Int` | first index at or after `from` **not** in the set |
| `bytes_scan_until` | `(Bytes, Int, Bytes, Int) -> Int` | first index at or after `from` **in** the set |
| `bytes_position` | `(Bytes, Int, (Int) -> Bool / e) -> Option<Int> / e` | the early-exiting find; the `List` counterpart is `iterate` |

`bytes_scan(b, from, set, max)` takes the byte class as a **`Bytes` of its
members** — `b"0123456789"`, `b" \t"` — so there is no closed enum to extend, and
returns `min(bytes_len(b), from + max)` when it did not stop, so the caller
distinguishes a class ending from a budget running out. `max` is what stops a
20-megabyte header line being a denial of service.

Cost model, which is the point of the section:

| builtin | time | allocation |
| --- | --- | --- |
| `bytes_index_of`, `..._from` | `memchr::memmem`, SIMD with a skip table | one `Value` for the `Option` |
| `bytes_index_of_byte` | `memchr::memchr`, SIMD | one `Value` |
| `bytes_starts_with`, `bytes_ends_with` | O(min(n, m)), exits at first mismatch | none |
| `bytes_split` | O(n) over `memmem::find_iter` | one `Value::Bytes` **copy** per piece |
| `bytes_scan`, `bytes_scan_until` | O(min(max, n − from)) over a 256-bit bitmap built in O(len(set)) | **none per byte** |
| `bytes_position` | O(bytes examined) | one boxed `Int` and one frame **per byte examined** |

`bytes_position` is the escape hatch: prefer `bytes_scan` wherever the predicate
is a byte set, because `bytes_position` pays exactly the cost W1 measured,
reduced by early exit rather than removed. `bytes_split` copies because
`Value::Bytes` is `Arc<[u8]>` with no slicing, which ADR 0011 deferred to W3.

The prover treats every one of these as opaque. The total ones are in
`TOTAL_BUILTINS`; `bytes_index_of_from`, `bytes_index_of_byte`, `bytes_split`,
`bytes_scan`, `bytes_scan_until` and `bytes_position` refuse an input and are
deliberately not.

#### The re-measurement — **landed**

`examples/hello.ply`'s parser is written with these, and
`cargo run --release -p ply-corpus -- serve --repo .` is the number. One
thread, the machine engine, the benchmark's 63-byte head:

| | W1's folds | these builtins |
| --- | --- | --- |
| `answer` alone — parse and response build | 358 µs | 48 µs |
| a whole request over a real socket | 714 µs | 221 µs |
| requests per second, one thread | 1401 | 4528 |
| share of a served request above the socket | 86% | 34% |

The ratio is not the claim; the **shape** is. `serve` gained a head-length
sweep — the same `answer` over heads grown from 23 bytes to 1943 by adding
header lines the parser never reads, so every point parses the same three
fields:

| head | W1's folds | these builtins |
| --- | --- | --- |
| 23 bytes | 216 µs | 50 µs |
| 1943 bytes | 8458 µs | 47 µs |

84 times the bytes cost **39 times** the time before and **0.95 times** the
time now. A request's cost has stopped being a function of how long its head is
and become a function of how many fields were parsed out of it, which is the
exit criterion ADR 0011 states.

> **Audit note: ADR 0011 reports this same sweep as 29.29x → 1.00x, and this
> same request as 527.7µs → 109.8µs at 1,895 → 9,109 req/s.** Both documents
> claim to be quoting W2. They cannot both be. This section is the one with a
> rig attached, so it is the better of the two — but it is not confirmed either.
> Re-running exactly the command named above
> (`ply-corpus serve --repo . --baseline`) for the audit produced, on a slower
> box: `answer` alone **316.1 µs → 23.8 µs**, a whole request over a real socket
> **617.4 µs → 135.6 µs**, **1,620 → 7,372 req/s**, share above the socket
> **78% → 32%**, and a head sweep the tool itself prints as **84x the bytes cost
> 26.27x the time** under W1's folds and **0.93x** under the byte builtins.
>
> The absolute µs are machine-dependent and the audit's box is slower throughout,
> so those are expected to move. The sweep *ratio* is not supposed to be — it is
> a shape claim about an O(n) parser — and the three recorded values for it are
> **39x** (here), **29.29x** (ADR 0011) and **26.27x** (re-measured). Note
> also that the tool computes that ratio itself, as last-row µs over first-row
> µs; **39** is what this table's own two rows divide to (8458/216 = 39.2), so
> this section is at least self-consistent, whereas ADR 0011's 29.29x has no
> table under it anywhere. The qualitative claim — the second column is flat and
> the first is not — reproduces on every take. A faster interpreter would have divided the
first column; it would not have flattened it. What is left above the socket is
`handle` dispatch and the response build, and the socket is now two thirds of a
served request — which is the input W6 wanted and not the one M9 assumes.

### Versions

`RUNTIME_VERSION` to `0.7.0` — **landed** — (`Value` gains three variants and
the evaluator gains the builtins above). `FRONTEND_VERSION` to `0.9.0` — **landed** — for
`FnDef::constraints`, `Lit`'s two new variants, the prelude's four new types, and
the derivers' output. `BODY_ENCODING` to `5` — **landed** — for
`tag::CONSTRAINT`, `LIT_FLOAT` and `LIT_DECIMAL`. `PROVER_VERSION` to `0.3.0` for
the new generators.

### Workspace

One new crate, `ply-std`, and two new dependencies:

```toml
ply-std = { path = "crates/ply-std" }
memchr = "2.8.3"
rust_decimal = "1.42.1"
```

`ply-eval` gains both. ADR 0011 said `ply-eval` gains no dependency; W2 reverses
that for two crates whose entire content — a SIMD substring search, an exact
decimal — would be foolish to write here, and neither brings a runtime, a
reactor, or a value type that enters `Value`. `bigdecimal`, `ryu` and `itoa` were
considered and rejected; the reasons are in the ADR. `ply-std` depends on
`ply-span` and `ply-syntax` only.

### New diagnostic codes — landed

Added to `ply_span::codes`; existing numbers are unchanged and the registry pin
test covers all six.

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0113 | `RESERVED_MODULE_NAME` | a project file whose module name would be `std` or under it | the program's |
| E0206 | `NOT_DERIVABLE` | `derivable(D, t)` does not hold — at a `derive`, at a constrained call site, or at a `Map` key type | the program's |
| E0207 | `UNKNOWN_DERIVER` | a `derive` or `where` naming something that is not a deriver | the program's |
| E0208 | `ORPHAN_DERIVE` | a `derive` outside the module declaring its target | the program's |
| E0209 | `DECIMAL_DIVISION` | `/` applied to `Decimal` | the program's |
| W0605 | `STDLIB_CHANGED` | the cache was written under a different stdlib digest | nobody's |

`E0206` covers three shapes because they are one claim and a consumer's response
to all three is the same.

### Required tests

The full list is ADR 0011's; these are the ones whose absence would let W2 ship
broken rather than merely incomplete.

1. A program importing nothing from `std` has hashes byte-identical to before.
2. Copying a `std` module's source into a project produces **identical**
   `DefHash`es and shares its cache entries.
3. Changing one `std` definition re-selects exactly the tests reaching it and no
   others; changing none re-runs nothing.
4. A project file at `std/json.ply` is `E0113`. A `std` module importing a
   project module is `E0505`.
5. `map_keys` is ascending over 10,000 random insertion permutations of one key
   set.
6. `Value::cmp(a, b) == Equal` iff `values_equal(a, b)` over the generator's
   whole range, with the `Float` NaN exception asserted rather than excluded.
7. Two maps built in different orders are `values_equal`, and a test asserting so
   is cached under one order and read from cache under the other.
8. `Map<Float, v>` is `E0206`; `Map<k, v>` under an unconstrained `k` is `E0206`
   naming the clause to add, and adding it fixes it.
9. **Renaming a derived type re-runs no test; renaming a variant re-runs exactly
   the tests reaching it.** Both, on one corpus, in one test.
10. Reordering two fields changes the generated definition's hash.
11. A type with a function field is `E0206` naming the field.
12. A generated definition and a hand-written one with the same normalized form
    have the same hash.
13. Adding a `where` clause changes the hash; reordering two does not; renaming
    the constrained parameter does not. **Landed** in `ply-hash`.
14. `where derivable(json, a)` fails at the **call site**, with the signature as a
    secondary label.
15. The deriver's golden output pin fails when the deriver changes and says to
    bump `FRONTEND_VERSION`.
16. `1`, `1.0` and `1m` are three definitions; `0.0` and `-0.0` are two.
17. `total / count` on `Decimal` is `E0209` naming `decimal_div`; a `Decimal`
    addition that overflows is `RUNTIME_ERROR` rather than a wrap or a rounding.
18. **A law mentioning a `Float` is never `proved`**, including a trivially true
    one, and the differential prover audit covers it.
19. A JSON number that does not fit a `Decimal` is a decode error naming the byte
    offset.
20. `bytes_scan` never examines more than `max` bytes, asserted by a counting
    harness rather than by timing; `bytes_position` calls its predicate once for a
    match at index 0 of a megabyte buffer.
21. `--audit-backend` on a corpus using every new builtin reports no `E0503`.
22. `Store::open` at 10,000 definitions stays under 5 ms.
23. Incremental and `--no-incremental` agree byte-for-byte over a corpus using
    every W2 feature, across the full mutation sequence.
24. Renaming a top-level function still selects zero tests, and moving a
    definition between modules still changes no hash, on a corpus with
    derivations and stdlib imports.

Plus one `tests/fixtures/` entry per new code.

### Not in W2

Type-directed dispatch — ADR 0010 decision 6 stands, so `++` stays `String`-only
and `len` stays `(List<a>) -> Int`. `derive row`, which lands in W4 with the
`Row` type. User-defined derivers, conditional instances, higher-kinded types. A
map literal. Byte-slice patterns and cheap slicing of a shared `Bytes`, both W3.
A JSON number outside `Decimal`'s range. Top-level value definitions.

And one wart, recorded rather than left to be rediscovered: **`string_find`
returns an `Int` and raises when absent**, which is W1's shape from before the
prelude had `Option`. The new byte builtins return `Option<Int>`. Changing
`string_find` would move every hash that uses it for no behavioural gain, and a
second name for one operation is worse than the inconsistency. W3 may unify them.

---

## A real server

`docs/adr/0011-the-web-track.md` has the reasoning; this section is the contract.
**Where it disagrees with any section above, this section wins** — it was
written after them.

### The rule everything else follows from

The trusted computing base does not grow to hold a protocol. HTTP/1.1 framing
is a pure function from bytes to a request, so `std.http` is **Ply source** —
`ply test` selects it exactly, its row is `{}`, and every framing rule below is
a hermetic `det` test rather than a line in `ply hosts` nobody can reach. TLS is
the one exception, because writing cryptography here would be reckless, and it
is the only place W3 adds to the TCB.

### Effect-set aliases — AST landed in `ply-syntax::ast`

```rust
/// `effect set Web = { db.read[users], log.write, Inner }`.
pub struct EffectSetDef {
    pub name: Ident,
    pub atoms: Vec<AtomExpr>,
    pub includes: Vec<QName>,
    /// Every atom this set denotes, after expanding `includes` transitively:
    /// sorted and deduplicated by written form, so that reordering the members
    /// or splitting one set into two produces the same expansion — which is
    /// what `--explain` prints and what a reader diffs.
    pub expansion: Vec<AtomExpr>,
    pub span: Span,
}

pub enum Item { /* ... */ EffectSet(Box<EffectSetDef>) }

pub struct RowExpr {
    pub atoms: Vec<AtomExpr>,
    /// The sets this row was written with, in source order. Provenance for
    /// `--explain`, **erased by normalization**: a row written `{Web}` and one
    /// written with `Web`'s expansion are one definition.
    pub aliases: Vec<QName>,
    pub tail: Option<Ident>,
    pub span: Span,
}
```

Grammar:

```
item        := "pub"? (fnDef | typeDef | effectDef) | testDef | lawDef
             | deriveDef | effectSetDef
effectSetDef:= "effect" "set" IDENT "=" "{" setMember,* "}"
setMember   := atom | qname
row         := "{" rowMember,* ("|" IDENT)? "}" | IDENT
rowMember   := atom | qname
```

A row member is an atom when its identifier is followed by `.` and a set
reference otherwise — one token of lookahead, no reserved word. A whole row that
is a bare `IDENT` is still a row *variable*; a set is only ever written inside
braces.

- **A member is an atom, never a whole effect.** ADR 0009's
  `effect set Web = {db, http}` is refused. "Every atom of `db`" is every
  resource label anywhere in the program, so an unrelated table in an unrelated
  module would change the expansion — and therefore the declared row, and
  therefore the hash — of every definition annotated with it, which is ADR 0011
  corollary 1. A wildcard atom is refused for the same weight of reason: it
  would put a non-ground shape into `EffectAtom`, which is what
  `conflicts_with` and every scheduling decision are built on. Keeping members
  atomic also keeps the resources visible where a reviewer reads the set, which
  is the only mechanism ADR 0009's "over-broad alias" risk has.
- **Sets are module-local.** Not `pub`, not importable; `other::Web` in a row is
  `E0114`. Gate 1 skips a file whose bytes are unchanged, so a set expanding
  across a module boundary would let an edit in the declaring module leave a
  stale published row behind — a footprint that under-reports, which is a
  **green** result. ADR 0011 records the mechanism a sound cross-module
  form would need (`ImportEdge::exports`, a `DefKind::EffectSet`, a fourth
  namespace, and a hygiene rule for substituted effect names) so that it is a
  deferral rather than an omission.
- **Expansion happens inside `ply_syntax::parse_module`**, before it returns, so
  an unexpanded `RowExpr` never escapes the parser and no crate can forget to
  run it. It reads this module's own `effect set` items **and nothing else**,
  which is what makes it a function of the file, which is what makes gate 1
  right. It runs **before** `ply_derive::expand_module`.
- `Item::EffectSet` declares no name a reference can reach and generates no
  definition: `Item::name`, `Item::visibility`, `resolve::declarations_of`,
  `graph`, `interp`, `machine`, `infer` and the driver all skip it, exactly as
  they skip `Item::Derive`. A set name lives in no namespace `resolve` knows
  about, so `effect set Web` beside `type Web` is legal.

**The three properties.** An alias is annotation-only: expansion produces an
ordinary `RowExpr`, `Checker::signature` converts it, and `check_upper_bound`
checks the inferred row against it through the existing path. Nothing downstream
sees an alias — scheduling, isolation, reduction, `E0412` and the cached
footprint all speak in atoms. An `E0302` against an aliased signature quotes the
**expansion** in its secondary label, never the name.

**An alias name never enters a hash; its expansion enters exactly as the row it
stands for would.** ADR 0009's "regrouping which atoms it contains must
change no definition hash" is superseded:

| edit | hashes that move |
| --- | --- |
| declaring a set nothing uses; renaming one; reordering its members | none |
| rewriting `/ {db.read[users], log.write}` as `/ {Web}` expanding to exactly that | **none** — the headline test |
| changing which atoms a set contains | exactly the definitions annotated with it, and their transitive dependents |

The last row is required rather than conceded. `Signature::published_row` is the
declared row and `DefInfo::footprint` is what callers are inferred against, so
widening a set widens a published bound — and gate 2 only rechecks a definition
whose own hash moved. A set edit that moved no hash would leave a caller
accepted against a signature that no longer admits it, and its stored footprint
under-reporting what it can reach. Same argument as ADR 0011's `where`
clauses, same answer.

`ply_hash::normalize::row` already sorts and deduplicates atoms, so both
spellings write the same bytes. **`BODY_ENCODING` does not move**, and a corpus
with no `effect set` hashing byte-identically to W2 is a required test.

One correction was needed to make that true. `normalize::row` sorted the atoms'
*bytes* but committed the references they mention in written order, and
first-mention order is what numbers the effect slots those same bytes carry — so
`/ {db.read[users], log.write}` and `/ {log.write, db.read[users]}` were two
definitions, which contradicts both the function's own comment and the rule that
reformatting is free. A row now holds its mentions back until the sort has run
and commits them in sorted order. This is a defect that predates `effect set`
and is visible without one; an alias only made it routine, since a set's
expansion is spliced in wherever the set is named, beside atoms written by hand.
No pinned hash in the workspace moved, because every one of them was already
written in the order the sort produces.

**What an over-broad alias costs, published rather than worried about.** A
declared row wider than the inferred one is legal and an alias makes it
systematic. Two mechanical costs follow: `DefInfo::footprint` is the declared
row, so two endpoints sharing a set contend on every atom in it and the
scheduler serialises tests that need not be; and DESIGN.md §7 makes the
footprint the frame condition, so a wider row makes every `ensures` on that
definition promise less. So:

```rust
pub struct DefInfo { /* ... */
    /// The row inference computed for the body — always a subset of
    /// `footprint`, and equal to it when the definition carries no annotation.
    /// Provenance: in no hash, no cache key, no scheduling decision and no
    /// determinism verdict.
    pub performed: Footprint,
    /// The `effect set` names this definition's row was written with, in source
    /// order. Erased by normalization.
    pub row_aliases: Vec<Symbol>,
}
pub struct KnownDef  { /* ... */ pub performed: Footprint }
pub struct CachedDef { /* ... */ pub performed: Footprint, pub row_aliases: Vec<Symbol> }
```

Both are stored on `CachedDef`, because `DefInfo` is restored from it when gate
1 skips a file and `ply check --types --explain` must print the same bytes warm
and cold. A reviewing command whose output depends on what the cache held is a
reviewing command that stops diffing against itself.

`KnownDef` carries only `performed`. It is the gate-2 path, where the file *was*
parsed and its `RowExpr::aliases` are right there — a stored copy would be a
second answer to a question the AST already answers, and the two could disagree.

**`ply check --types` prints the expansion, always, and never the alias** —
ADR 0009 in its strongest form, with the truth behind no flag at all.
`--explain` adds the set table, the alias a row was written with, the body's
inferred row, and the declared-but-not-performed difference. `ply prove` prints
the same difference for a definition carrying an obligation, where it is a
weakened claim rather than a scheduling cost.

### HTTP/1.1 — `std.http`, Ply source

```ply
type Method  = Get | Head | Post | Put | Patch | Delete | Options | Other(String)
type Version = Http10 | Http11
type Headers = Map<String, List<String>>

type Request = {
  method: Method, target: String, path: String, query: String,
  authority: String, version: Version,
  headers: Headers, trailers: Headers, body: Bytes,
}
type Response = { status: Int, headers: Headers, body: Bytes }

type Framing  = NoBody | Length(Int) | Chunked
type Refusal  = { status: Int, reason: String }
type Head     = { request: Request, framing: Framing, consumed: Int,
                  keep_alive: Bool, expects_continue: Bool }
type HeadResult = Incomplete | Parsed(Head) | Refused(Refusal)

pub fn parse_head(buf: Bytes, limits: Limits) -> HeadResult        // row {}

type BodyStep =
  | Await({ state: BodyState, consumed: Int, out: Bytes })
  | Complete({ consumed: Int, out: Bytes, trailers: Headers })
  | Rejected(Refusal)
pub fn body_start(framing: Framing, limits: Limits) -> BodyState   // row {}
pub fn body_step(state: BodyState, buf: Bytes) -> BodyStep         // row {}

type BodyResult = Body({ bytes: Bytes, rest: Bytes, trailers: Headers })
                | BodyRefused(Refusal)
pub fn read_body(conn: Int, framing: Framing, buf: Bytes, limits: Limits)
  -> BodyResult / {net.write[conn]}

pub fn encode(method: Method, version: Version, keep_alive: Bool, r: Response) -> Bytes
pub fn method_not_allowed(ms: List<Method>) -> Response        // the 405, with `Allow`
pub fn respond_chunked<s | e>(
  conn: Int, version: Version, r: Response, keep_alive: Bool, limits: Limits,
  seed: s, produce: (s) -> Option<{ chunk: Bytes, next: s }> / e
) -> Bool / {net.write[conn] | e}

pub fn serve_connection<e>(conn: Int, limits: Limits, app: (Request) -> Response / e)
  -> Unit / {net.write[conn] | e}
pub fn serve<e>(listener: Int, limits: Limits, app: (Request) -> Response / e)
  -> Unit / {net.write[listener], net.write[conn] | e}
```

Field names are lowercased on parse; a name may repeat, so the value is its
field lines in order of appearance and **nothing is combined** — a comma-joined
`set-cookie` is a different document. `Headers` is a `Map`, so its order is
ascending and canonical and a golden test over response bytes is stable.

`consumed` is where the body begins, so a pipelined second request is a slice
rather than a re-parse. A streamed response needs no language feature, only a
row variable: `produce` is an ordinary function with its own row.

**Every connection `serve` handles shares the resource label `conn`**, because a
resource label is a ground identifier in the source. Two connections therefore
conflict — which costs nothing inside a run, since a production region schedules
on real readiness, and does mean two *tests* that each serve a connection land
in one concurrency group.

### Framing — implement exactly this

Every rule closes the connection, and every one is a required test.

| # | input | answer |
| --- | --- | --- |
| 1 | a bare LF terminating a line, or a bare CR in the header block | 400 |
| 2 | obs-fold (a line beginning SP/HTAB) | 400 |
| 3 | whitespace between a field name and its colon | 400 |
| 4 | a field name that is not a token, or a value with a CTL other than HTAB | 400 |
| 5 | a method that is not a token | 400 |
| 6 | authority-form target; `*` with anything but `OPTIONS` | 400 |
| 6a | SP or HTAB anywhere in the request line beyond the two that split it | 400 |
| 6b | a request target carrying a fragment (`#`), in either form | 400 |
| 6c | an absolute-form target whose authority is empty or carries userinfo (`@`) | 400 |
| 7 | no version in the request line | 400 |
| 8 | `HTTP/x.y` other than 1.0 or 1.1 | 505 |
| 9 | request line over `max_request_line` | 414 |
| 10 | HTTP/1.1 with no `Host`, or with two | 400 |
| 11 | **two `Content-Length` field lines, even with equal values**; `Content-Length: 5, 5` | 400 |
| 12 | `Content-Length` that is not one or more ASCII digits | 400 |
| 13 | `Content-Length` over `max_body` | 413, before a body byte is read |
| 14 | **`Content-Length` and `Transfer-Encoding` both present** | 400, no body read |
| 15 | `Transfer-Encoding` whose final coding is not `chunked` | 400 |
| 16 | a transfer coding other than `chunked`, **including beside `chunked`** | 501 |
| 17 | chunk size empty, non-hex, or over 16 hex digits | 400 |
| 17a | a chunk size that does not fit in an `Int` | 413 |
| 18 | chunk size over `max_chunk_size`; chunk data summing over `max_body` | 413 |
| 19 | a chunk-size line over `max_chunk_line` | 400, without buffering past the bound |
| 20 | a chunk not followed by CRLF | 400 |
| 21 | header block over `max_header_bytes`, or over `max_header_count` | 431 |
| 22 | the head not complete within `header_timeout_ms`, measured from its first byte | 408 |
| 23 | the body not complete within `body_timeout_ms` | 408 |
| 24 | `idle_timeout_ms` between requests | close, no response |

Rule 15 is decided before rule 16, which is the one place their order is
observable: `chunked, gzip` has no decidable length and is 400, while
`gzip, chunked` is framed unambiguously and is 501 — accepting it would hand the
handler undecoded `gzip` as `Request::body` with nothing saying so, and would
leave an intermediary that honours the coding and a server that ignores it
disagreeing about what the body was.

Rule 17a is not a refinement of rule 17. Sixteen hex digits is sixty-four bits
and `Int` is signed, so a size at or above `8000000000000000` overflows the
accumulator — and an overflow is `E0502 RUNTIME_ERROR`, which is not a `Refusal`:
it unwinds out of `body_step`, out of `serve_connection` and past the accept
loop. The digit bound alone is therefore a remote kill, and the size is checked
for representability before it is accumulated.

Rules 6a, 6b and 6c each exist because the alternative is two readings of one
target. RFC 9112 §11.2 names recovering from whitespace in the request line as a
smuggling vector; a fragment is no part of a request-target and under
absolute-form would otherwise *become* `Request::path`, which every consumer
reads as beginning with `/`; and RFC 9110 §4.2.4 says to treat userinfo in an
`http` URI from an untrusted source as an error, which a request-target is.

Rules 11 and 14 are where W3 refuses what the RFC permits, and both refusals are
the anti-smuggling ones: agreement between two `Content-Length` lines in *this*
message says nothing about how a hop in front of the server picked one, and
preferring `Transfer-Encoding` is correct only if every hop made the same choice.

Under absolute-form the target's authority is the request's `authority` and
`Host` is not consulted for it (RFC 9112 §3.2.2) — **both are exposed as data**,
because whether a mismatch matters is the program's question, not the parser's.

Chunk extensions are parsed for framing and discarded. **Trailers are exposed as
`Request::trailers` and never merged into `headers`**: a trailer
`content-length`, `transfer-encoding`, `host` or `authorization` changes no
framing, routing or authorization decision. `Content-Encoding` is end to end and
is passed through untouched; W3 decodes nothing.

Keep-alive: 1.1 persistent unless `Connection: close`; 1.0 closes unless
`Connection: keep-alive`; **never reused after any `Refusal`**; closed after
`max_keep_alive` requests; **an unconsumed request body is drained to `max_body`
and the connection closed past that**, because reading the next request out of
an unread body is a smuggle the server performs on itself. Pipelined requests
are answered in order by construction — one connection, one task, one carried
buffer.

`encode`: 1xx, 204 and 304 carry no body and no `Content-Length`; a `HEAD`
response carries the `Content-Length` a `GET` would and no body; a `100
Continue` is written before the body is read when the request declared
`Expect: 100-continue`, and any other `Expect` is `417`. **A header name that is
not a token, or a value containing CR or LF, is `E0502 RUNTIME_ERROR` naming the
header** — never stripped, never escaped: that value came from the program, and
silently sanitizing a response-splitting attempt leaves an attacker in partial
control of a response nobody looked at.

### Limits

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

A record, because a limit set is data like a route table: quantifiable in a
`forall`, derivable, printable. No global, no environment variable and no flag,
so two runs of one program cannot differ in what they refuse. A field that is
zero or negative is `E0502 RUNTIME_ERROR` at `serve`, naming it.

`max_stream_chunks` is a bound on a *write*: the most chunks one
`respond_chunked` may produce. It is here rather than in a constant because every
bound a program runs under belongs in this record, and it is a bound at all
because `stream_chunks` is a tail call charged against the evaluator's
nested-call budget. Exhausting it **terminates the message** — the terminating
chunk is written — and answers `false`, so the connection is not reused. A
chunked response left framed and unterminated on a connection a caller keeps
alive has its next response read as chunk data by the client, which is the
response-side spelling of the framing disagreement §3 exists to prevent.

**A deadline is enforced by dividing it, because Ply has no clock.** Rules 22 and
23 measure from the first byte, and passing `header_timeout_ms` whole to each
`net.recv` — which `ply_host::tcp::recv` turns into one `set_read_timeout` per
syscall — restarts the deadline on every byte, leaving the read count as the only
real bound: 2048 reads x 5000 ms is 2.8 hours on one socket, while `serve`, a
sequential accept loop, accepts nothing else. So each read carries
`timeout / max_reads()` and the budget is the number of slices, which bounds the
whole message by its deadline. A slice expiring with no bytes is an ordinary
read, not a refusal; the refusal is the budget running out. The wait for the
first byte of a message on a *reused* connection is one read carrying the whole
`idle_timeout_ms`, which is rule 24 and is a deadline before anything is being
assembled.

**The bound must cost the bound.** Every scan passes a `Limits`-derived `max` to
`bytes_scan` / `bytes_scan_until`, and the read loop searches for the header
terminator with `bytes_index_of_from` starting three bytes before the previous
end rather than from zero. W2 made a request's cost proportional to fields
rather than to bytes; a parser re-scanning the accumulated buffer per read
restores O(n²) quietly, and the test for it is a counting harness, never a
stopwatch.

One new builtin, for the other direction — N reads concatenated pairwise is
O(total²):

| builtin | type | notes |
| --- | --- | --- |
| `bytes_concat_all` | `(List<Bytes>) -> Bytes` | pure; one allocation, O(total); the empty list is `b""` |

**Cheap slicing of a shared `Bytes` is not in W3.** ADR 0011 deferred the
question here and the answer is no: with `bytes_concat_all` the read loop is
O(total) once and the head/body split is one copy, so a slice representation
buys a constant factor nothing has measured at the price of changing `Value`'s
one enum and every path that matches on it. W6 has the measurement.

### Routing — `std.router`, Ply source

```ply
type Segment  = Literal(String) | Param(String) | Rest(String)
type Route<a> = { method: Method, path: List<Segment>, endpoint: a }
type Matched<a> =
  | NotFound
  | MethodNotAllowed(List<Method>)                       // sorted, deduplicated
  | Found({ endpoint: a, params: Map<String, String> })

pub fn route<a>(table: List<Route<a>>, method: Method, path: String) -> Matched<a>
pub fn conflicts<a>(table: List<Route<a>>) -> List<{ first: Int, second: Int }>
pub fn well_formed<a>(table: List<Route<a>>) -> List<{ index: Int, reason: String }>
```

All three publish `{}`. `std.router` imports `std.http` for `Method`; the edge
never goes the other way.

**The endpoint is a tag, not a closure**, and the reason is not taste: `List` is
homogeneous, a function type carries its row, and Ply has no subsumption — so a
table of closures would force every handler to declare byte-identically the same
row, which is the union of the whole service, which destroys exactly the
per-endpoint legibility this milestone exists to produce. With a tag the program
writes its own `match`, and that `match` is exhaustiveness-checked (`E0205`), so
**a route in the table with no handler is a compile error**.

Matching:

- the path is the target up to the first `?`, split on `/`, with no empty first
  segment for the leading `/`;
- **empty segments are kept** — `/orders/` and `/a//b` are not normalized;
  `normalize_path` exists and is a call the program makes. A silent
  normalization is a second answer to "which path is this", and two answers is
  how a route and an authorization check come to disagree;
- **percent-decoding is per segment, after splitting**, so `%2F` decodes to a
  `/` inside one segment and can never introduce a boundary. An invalid escape
  is left as written;
- **decoding costs the segment's length and never its square.** `route` reaches
  `percent_decode` for every request before it has decided anything, so an
  accumulator that copies — `push` onto a `List` does — makes k escapes cost
  O(k²), which was 125.8 ms for a 7,681-byte path of escapes against 0.1 ms for
  the same-length plain path, at a length the default `max_request_line` admits.
  Decoding is one native split on `%`, one call per escape and one allocation for
  the join; encoding is one native scan that answers for a segment needing none.
  Neither recurses per escape, so neither can end a run at the interpreter's
  nested-call limit. This is required test 26's cost property, for routing;
- byte-exact and case-sensitive; `Rest` matches zero or more remaining segments
  and must be last.

Precedence is decided **segment by segment, left to right**: at the first
position where two patterns differ in kind, `Literal` beats `Param` beats
`Rest`. A remaining tie goes to the earlier entry — and is a defect, which
`conflicts` reports so a service can write `test "the route table is
unambiguous" { assert_eq(conflicts(table()), []) }`. That is what "the route
table as ordinary data" is for: the property a framework enforces with a macro
is here a value a test asserts.

`MethodNotAllowed(ms)` carries the sorted, deduplicated methods that do match,
and `std.http.method_not_allowed(ms)` builds the `405` with `Allow:` so a
program cannot forget the RFC 9110 §15.5.6 MUST.

### TLS

**TLS is not a separate effect.** It is `net`, with one new operation.

A separate `tls` effect with the same five operations makes every function that
touches a socket exist twice, because Ply cannot abstract over two effects that
declare the same operations — so `std.http` would fork, and two forks of a
framing parser is two parsers that disagree. A row claims which resources are
touched and whether two computations contend; a TLS connection and a plaintext
one are the same resource and contend the same way. Encryption is a property of
the transport, not of the resource.

**So the row does not say whether a connection is encrypted; the listener
does**, and `ply hosts` prints `net.listen_tls` as its own line — a fact in the
TCB listing rather than an inference from a row.

**The key never enters the program.** `listen_tls` takes a credential *name*:
certificate bytes as a literal would put a private key into a definition hash
and a store designed never to forget, and a file path would put a file read
inside a `net` operation where nothing discloses it.

```
$ ply run service.ply --host --tls api=certs/api.pem,certs/api.key
```

Repeatable. PEM: a chain leaf-first, and a key in PKCS#8, PKCS#1 or SEC1.
Everything loads and validates at **bind time, before anything runs**.

- **The handshake is lazy — on the first `recv` or `send`, never inside
  `accept`.** A handshake in `accept` means one client sending garbage takes
  down the accept loop.
- A failed handshake **closes that connection and nothing else**: `recv` answers
  `Some(b"")` and `send` answers `Some(0)`, so the server's ordinary "peer went
  away" path handles it. It is not a diagnostic — not the program's fault, not
  Ply's, not attributable to a definition — and it is counted and reported in
  the run's `--host` summary and in `--json`.
- **ALPN offers exactly `http/1.1`.** A client offering only `h2` is refused at
  handshake rather than served 1.1 bytes over a connection it will parse as
  HTTP/2.
- One credential per listener. No SNI selection, no mTLS, no resumption, no
  OCSP.

`ply hosts --host` gains a `transport` block naming the library, its version,
its provider, the protocol versions and the ALPN offer, and a `credentials`
block naming each credential with its certificate chain's SHA-256 fingerprint
and length. **The digest covers the credential names, the provider and the
library version, and not the fingerprint**: a CI check that broke on every
renewal is a CI check people learn to ignore, while adding or removing a
credential is a structural change to the TCB and does move it.

### `net`, amended

```ply
pub nondet effect net {
  write listen[s](port: Int) -> Int
  write listen_tls[s](port: Int, credential: String) -> Int
  write accept[s](listener: Int) -> Int
  write recv[s](conn: Int, max: Int, timeout_ms: Int) -> Option<Bytes>
  write send[s](conn: Int, payload: Bytes, timeout_ms: Int) -> Option<Int>
  write close[s](socket: Int) -> Unit
}
pub fn send_all(conn: Int, payload: Bytes, timeout_ms: Int) -> Bool / {net.write[conn]}
```

**A peer's misbehaviour is not the program's error.** A reset, a broken pipe, an
aborted connection and a failed handshake are ordinary outcomes — end of stream
— not diagnostics that end the run. What stays a diagnostic is the program's
fault: an unknown handle, a handle used under two labels, a port outside
`1..=65535`, a non-positive read bound, a non-positive timeout, an empty `send`
payload. `accept` answers `0` when the listener is finished; handles ascend from
1 and are never reused, so `0` is never a live socket.

**A deadline is an argument, not a cancellation.** ADR 0011 deferred
cancellation and said W3's timeouts need it; they do not. A cancel path needs a
token registry, a race between cancel and completion, a rule for what a
cancelled operation returns and a decision about bytes already read; a deadline
is one `setsockopt` inside a blocking job that already owns the socket.

One rule: **`None` is a deadline; an empty `Some` is an ending.** `recv` →
`None` timed out, `Some(b"")` peer stopped sending, `Some(bs)` those bytes.
`send` → `None` timed out, `Some(0)` peer gone, `Some(n)` n written.
`timeout_ms <= 0` and an empty `send` payload are each `RUNTIME_ERROR`, the
second being what keeps `Some(0)` unambiguous. Programs call `send_all`.

These signature changes move `std.net`'s hashes and everything reaching them,
which is correct: the signature changed, and selection is exact about it.

`MAX_BLOCKING_OPERATIONS` is unchanged at 64 — one real thread per waiting
operation, so 64 socket operations in flight is the capacity of the W3 server.
A number a reviewer can read; raising it or moving to a reactor is W5/W6 with a
measurement.

### Versions

`RUNTIME_VERSION` to `0.9.0` — `bytes_concat_all`, and `net` handlers answering
`Option` where they answered a bare value. `FRONTEND_VERSION` to `0.11.0` —
`RowExpr::aliases`, `Item::EffectSet`, expansion inside the parser, and
`DefInfo` / `KnownDef` / `CachedDef` gaining `performed` and `row_aliases`.
**`BODY_ENCODING` stays at `6`** and `PROVER_VERSION` stays at `0.4.0`; a corpus
with no `effect set` hashing byte-identically to W2 is a required test, not an
observation.

### Workspace

```toml
rustls = { version = "0.23.43", default-features = false, features = ["ring", "std", "tls12"] }
rustls-pemfile = "2.2.0"
rcgen = "0.14.9"      # dev-dependency of ply-host only
```

`0.23.43` is the latest stable 0.23; `cargo search` surfaces `0.24.0-dev.1`,
which is a pre-release and must not be used. `ring` rather than `aws-lc-rs`
because the latter needs a C toolchain and cmake on some platforms, and the
provider is **installed explicitly** rather than taken from a default feature,
so the one line that decides it is the line `ply hosts` names. `tokio-rustls` is
**not** used: W1's sockets are blocking `std::net` on a pool `ply-host` owns, so
`rustls::StreamOwned` is the fit and `tokio-rustls` would need an async socket
layer nothing here has. While in that file — **`tokio` is a declared dependency
of `ply-host` that no code uses**; remove it or use it, because a dependency in
a trusted computing base that nothing calls is attention spent for nothing.

`ply-std` gains `std.http` and `std.router` and no dependency; `ply-eval` gains
one builtin and no dependency.

### New diagnostic codes — landed

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0114 | `UNKNOWN_EFFECT_SET` | a row or a set names an `effect set` this module does not declare; a qualified set reference; `pub effect set` | the program's |
| E0115 | `EFFECT_SET_CYCLE` | a set contains itself, directly or through another | the program's |
| E0429 | `TLS_CREDENTIAL_UNKNOWN` | `net.listen_tls` named a credential the binding does not hold | the run's configuration |
| E0430 | `TLS_CREDENTIAL_INVALID` | a `--tls` credential that does not load: unreadable, malformed PEM, no certificate, no key, or a key that does not match the leaf | the run's configuration |

Two `effect set`s with one name in one module are `E0105`. **Nothing in the
HTTP, limits or routing sections has a diagnostic code**, and that is the point
of writing the protocol in Ply: a malformed request is a `400`, which is a
`Response` value with no compiler involvement at all.

### Required tests

The full list is ADR 0011's; these are the ones whose absence would let W3
ship broken rather than merely incomplete.

1. A definition written `/ {Web}` and one written with `Web`'s expansion have
   **identical** `DefHash`es; renaming a set, reordering its members and
   declaring an unused one each move no hash and select zero tests.
2. Changing which atoms a set contains moves exactly the annotated definitions'
   hashes and their dependents', and selects exactly the tests reaching them.
3. A corpus with no `effect set` hashes byte-identically to W2.
4. `ply check --types` prints the expansion and never the alias;
   `--types --explain` prints the set table, the alias, the inferred row and the
   declared-but-not-performed difference, with **identical bytes** whether gate 1
   parsed the file or skipped it.
5. `E0114` for an undeclared set, a qualified reference and `pub effect set`;
   `E0115` naming a cycle in order; `E0105` for a duplicate set name.
6. Framing rules 1–24 above, each its own test.
7. **The anti-smuggling property**: over a generated corpus of adversarial
   heads, every accepted head admits exactly one body length and its `consumed`
   agrees with a reference table.
8. **The cost property**: the W2 head-length sweep re-run against
   `std.http.parse_head` over heads grown to 8 KB of fields the parser never
   reads, flat in the head's length; and a counting harness showing no scan
   examined more than its `Limits`-derived bound.
9. A response header value containing CR or LF is `E0502` at `encode`, naming
   the header.
10. An unconsumed request body is drained to `max_body` and the connection
    closed past it; the next request is never framed out of an unread body.
11. `route`, `conflicts` and `well_formed` publish `{}`; precedence is literal
    over parameter over wildcard left to right; a tie goes to the earlier entry
    and `conflicts` reports it.
12. `%2F` decodes after splitting and never introduces a boundary; `/orders` and
    `/orders/` are different paths and both route.
13. A `match` over the endpoint tag missing an arm is `E0205`.
14. A request over TLS against an rcgen-generated certificate returns the same
    bytes as the plaintext path **with no change to the service's source**; a
    client offering only `h2` is refused at handshake.
15. A failed handshake leaves the accept loop running and is counted; `E0429`
    for an unconfigured credential, `E0430` before anything runs for one that
    does not load, `E0424` for `net.listen_tls` without `--host`.
16. `recv` → `None` is distinguishable from `Some(b"")`; a reset peer is end of
    stream and not a diagnostic; `send_all` answers `false` rather than looping.
17. Renaming a top-level function selects zero tests and moving a definition
    between modules changes no hash, on a corpus with effect sets, a route table
    and a TLS listener.
18. Incremental and `--no-incremental` agree byte-for-byte across the full
    mutation sequence, with `effect set` edits added.
19. `--audit-backend` reports no `E0503`; `E0412` still fires; `ply test` is
    hermetic without `--host` and says so; `Store::open` at 10,000 definitions
    stays under 5 ms.

Plus one `tests/fixtures/` entry per new code.

### Not in W3

A database (W4). Authentication, authorization, sessions and cookies. HTTP/2 and
HTTP/3 — ALPN advertising only `http/1.1` is the honest form of not having them.
A template language. Compression in either direction; `Content-Encoding` is
passed through untouched. mTLS, SNI-based certificate selection, session
resumption and OCSP. `Upgrade`, WebSockets and `CONNECT`; authority-form targets
are `400`. Cross-module `effect set`s, and effect sets over row variables.
Cheap slicing of a shared `Bytes` — ADR 0011 deferred the question to W3 and
the answer is no, with the measurement W6 would need to change it. Cancellation
of a `Pending` token: deadlines removed the need rather than deferring it again,
and a host operation with no deadline still blocks until it completes or the run
ends. Graceful shutdown and connection draining (W5). More than 64 host
operations in flight.

---

## Postgres

`docs/adr/0011-the-web-track.md` has the reasoning; this section is the contract.
**Where it disagrees with any section above, this section wins** — it was
written after them.

### The rule everything else follows from

**A row says which tables.** The reason to put a database behind an effect is
that an endpoint's declared signature names the tables it touches, and a driver
that answers `db.write[db]` for every statement has thrown that away and kept
only the ceremony. So the resource label is a table name, one statement's table
set is **computed and checked** rather than asserted, and every mechanism below
exists to keep the row honest when the thing that decides it — the SQL text — is
a runtime value rather than a piece of syntax.

### `std.db` — Ply source, the declaration

`crates/ply-std/ply/db.ply`, module `std.db`, effect `std.db.db`. It ships with
the compiler like `std.net`, so the signature the driver binds against and the
signature the program performs are one text that cannot drift.

```ply
pub nondet effect db {
  read  query[t](s: Stmt, ps: List<Param>)      -> Answer
  write execute[t](s: Stmt, ps: List<Param>)    -> Answer
  write returning[t](s: Stmt, ps: List<Param>)  -> Answer
  write begin(level: Isolation, access: Access) -> Answer
  write commit()                                -> Answer
  write abort()                                 -> Answer
  write rollback(reason: String)                -> Unit
}

pub type Isolation = ReadCommitted | RepeatableRead | Serializable
pub type Access    = ReadWrite | ReadOnly
pub type Stmt      = { sql: String }

pub type Param = PNull | PInt(Int) | PBool(Bool) | PText(String) | PBytes(Bytes)
               | PFloat(Float) | PNumeric(Decimal) | PJson(json::Json)
               | PArray(List<Param>)
pub type Cell  = CNull | CInt(Int) | CBool(Bool) | CText(String) | CBytes(Bytes)
               | CFloat(Float) | CNumeric(Decimal) | CJson(json::Json)
               | CArray(List<Cell>)
pub type Row   = Map<String, Cell>

pub type DbError = { code: String, constraint: String, detail: String }
pub type Answer  = Rows(List<Row>) | Count(Int) | Failed(DbError)
```

`nondet` is load-bearing and is the same sentence `std.net` carries: a `det`
test reaching an unhandled `db` operation is `E0412` with or without `--host`.
The twin discharges the atoms, which is what makes a twin-backed test `det`,
cached and hermetic.

**A SQLSTATE is a value, never a diagnostic.** A unique violation, a foreign-key
violation, a serialization failure, a connection that died mid-statement — each
is `Failed(e)` the program matches on, by ADR 0011's rule about a peer's
misbehaviour. `DbError::detail` carries the server's prose for a person and
**nothing ever compares it**; `code` and `constraint` are what a program and the
agreement law read.

### Transactions as handlers

```ply
pub type Rollback = { reason: String, error: Option<DbError> }
pub fn transaction<a, e>(level: Isolation, access: Access,
                         body: () -> a / {db.write | e})
  -> Result<a, Rollback> / {db.write | e}
pub fn sandbox<a, e>(body: () -> a / {db.write | e}) -> a / {db.write | e}
pub fn is_retryable(e: DbError) -> Bool
```

`transaction` is a `handle` whose **only** clause is `db.rollback`, and that
clause **does not resume**: its value is the value of the whole `handle`, so the
rest of the body — the rest of the function, its callers up to the boundary, the
statements it was about to issue — is the continuation and the continuation is
dropped. Nothing unwinds and no frame runs an epilogue. Zero resumptions
satisfies ADR 0008's `resumes <= 1` trivially, so rollback needs no exemption
from the linearity rule and no change to `handler::resume`'s check.

The data operations are **not** intercepted. A Ply clause names a concrete
`(operation, resource)` pair, so a transaction that intercepted them would need
one clause per table per operation and could not be a library function; it does
not need to, because a transaction is a scope and the only thing that must be
scoped in Ply is the abort. The driver routes statements onto the open scope's
connection from host-side state.

`db.begin` is `Linearity::AtMostOnce`, so a continuation captured before a
transaction opened cannot be resumed twice (`E0426`) — which is right, since the
replay would issue a second `BEGIN` inside one.

| exit | what happens |
| --- | --- |
| `commit()` | `COMMIT`; a deferred-constraint or serialization failure at commit is `Failed` and the scope closes rolled back |
| `db.rollback(r)` | the clause discards the continuation and issues `ROLLBACK` |
| the body **raises** | the raise propagates unchanged, nothing was committed, the scope is still open |
| the entry point ends with a scope open | `HostRuntime::end_entry_point` rolls it back |

**Two answers are deliberately not the bare server's, and both halves of W4 give
the same one.**

- A `commit()` of a transaction a statement already aborted is **`Failed`
  with `25P02`**. Postgres answers that `COMMIT` with the command tag `ROLLBACK`
  and no error, and `tokio_postgres::SimpleQueryMessage` does not carry the tag —
  so a driver that asked only whether the server errored reports `Count(0)`, and
  `transaction` evaluates to `Ok(value)` for a transaction whose every write is
  gone. The first `Failed` inside a scope poisons it and the close answers this;
  the twin does the same from the same flag.
- A `commit()` or `abort()` with **no scope open** is `Failed` with `25P01`. The
  server emits a `WARNING` and succeeds, and a warning is not a value a Ply
  program can read — so matching it would mean telling a caller its writes are
  durable when nothing was ever opened to hold them. The driver refuses locally
  and sends nothing; the twin answers the same.

A `Parse` inside an aborted block is `Failed` with `25P02` and **not** `E0433`:
postgres refuses `Parse` as well as `Execute` there, so a statement whose text a
connection has not cached would otherwise stop the run — and only for the
statements that happened to miss the cache.

```rust
pub trait HostRuntime { /* ... */
    /// Called by the machine on **every** exit path from an entry point — a
    /// value, a diagnostic, or a spent budget — before it resets. The driver
    /// rolls back every open scope and releases or discards its connections.
    ///
    /// `machine` is whose entry point ended. One `Postgres` serves the whole run
    /// and every worker gets a `Facilities` over it, so a teardown that closed
    /// everything would roll back a transaction another entry point is still
    /// writing into.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic>;
}
```

**A transaction scope is owned by `(MachineId, Option<TaskId>)`, not by the task
alone.** `Machine::performing_task` is `None` outside a production region, which
every `ply test` entry point is, and the runner drives one machine per worker
thread — so a table keyed on the task files every concurrently running entry
point under one key: one reads the other's uncommitted rows, and either one's
teardown ends the other's transaction. The scheduler cannot prevent it, because
transaction control carries the singleton `db.write` and only serialises
transaction-*opening* tests against each other.

The pool manager is the second lock: a connection returned with a scope open is
`ROLLBACK`ed on release, and one whose rollback fails is **closed and discarded
rather than returned**. A recycled connection carrying an open transaction makes
the next request read uncommitted rows of a request that already failed,
invisibly from either.

**Nesting is a savepoint**, not a refusal: `SAVEPOINT ply_sp_<depth>` /
`RELEASE` / `ROLLBACK TO`, bounded at `db_max_savepoints` (16), over-depth is
`Failed` with `54000`. Refusal was the alternative and is worse — a nested
transaction is what a helper called both standalone and from a larger operation
looks like, and refusing would make every such helper exist twice. A savepoint
has no isolation level, so a nested `begin` whose `level` differs from the open
scope's is `Failed` with `25001` naming both; a nested `ReadOnly` inside a
`ReadWrite` is accepted and its narrowing is **documentation, not enforcement**,
which is the only honest thing to say about it.

`ReadUncommitted` is not offered: postgres implements it as read committed, and
a name that promised dirty reads would be a name that lies. `access = ReadOnly`
issues `SET TRANSACTION READ ONLY`, so a write inside it is `25006` **from the
server** — a mechanical backstop on a read-only row, supplied by the one
component that cannot be fooled by an annotation.

`40001` and `40P01` are `Failed` and **W4 never retries**: only the program
knows whether the body sent an email between two statements. A retry is a fresh
call to `transaction`, not a second resumption, so it is outside the linearity
rule entirely.

A `db` operation from a task that does not own the open scope is **`E0436`**.
Both alternatives are wrong: sharing the connection is a protocol violation, and
quietly acquiring a second puts the statement outside the transaction its author
believed it was in. `E0425` already refuses a host operation in a re-executed
test, so a transaction is never explored by DPOR against real postgres — it is
explored against the twin, which is pure Ply, and that is where the roadmap's
"concurrent request races become findable" is actually delivered.

### Footprint granularity — the interesting problem

`db.query[items]` performs `(db, items, Read)`; `db.execute[items]` and
`db.returning[items]` perform `(db, items, Write)`. The label is the statement's
**principal table**, written at the call site, because resource labels are
ground identifiers in the source and the language has nothing else.

The transaction control operations take **no** resource, so their atom is the
singleton `db.write`. Stated rather than discovered: every definition that opens
a transaction carries it, so two tests that open transactions are serialised
even over disjoint tables. It is also true — they contend for one pool, which is
exactly the host state ADR 0008 says cannot be forked. Read-only endpoints
open no transaction and keep their concurrency. A program wanting finer
granularity writes its own `handle` over `db.begin` with its own labels, as ADR 0011 says about `conn`.

**A statement may touch more tables than its label names.** `select … from
orders join items` performed as `db.query[orders]` records one atom and touches
two, and nothing in the type system can see it because the SQL is a `String`.
Two answers are refused — one-table-per-statement makes a join inexpressible,
and a group label makes `db.write[db]` with more syllables — and the third is
taken: **the driver computes what a statement would touch and refuses before it
runs it.** The design below went further — the driver would *report* what it
touched and the machine would check the report — and that half did not ship.

> **`HostReply` does not exist.** There is no such type anywhere in `crates/`,
> `HostAnswer` still has the W1 variants, and `HostRuntime`'s three W1 methods
> still answer a `Value`. What that costs is stated after the block: the
> **preventer** landed and the **detector** did not, so the boundary is trusted
> exactly as far as the driver's own SQL scan reaches.

```rust
/// A completed host operation. NOT SHIPPED.
pub struct HostReply {
    pub value: Value,
    /// Every atom this operation touched **beyond** the one the registry
    /// resolved. Empty for every handler whose footprint is a property of its
    /// registration, which is every handler W1 and W3 shipped.
    pub touched: Footprint,
}
impl HostReply {
    /// The W1/W3 shape: a value, nothing touched beyond the resolved atom.
    pub fn value(value: Value) -> HostReply;
}

pub enum HostAnswer { Reply(HostReply), Pending(Pending) }   // NOT SHIPPED
```

**As shipped**, in `ply_eval::host`:

```rust
pub enum HostAnswer { Value(Value), Pending(Pending) }   // unchanged since W1

pub trait HostRuntime {
    fn poll(&self, p: &Pending) -> Result<Option<Value>, Diagnostic>;
    fn park(&self) -> Result<(), Diagnostic>;
    fn block_on(&self, p: Pending) -> Result<Value, Diagnostic>;
    /// Takes the machine whose entry point ended, exactly as the block above
    /// this section says it must — one `Postgres` serves the whole run.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic>;
    fn stopping(&self) -> bool;                       // W5
    fn shutdown(&self, drain_ms: u64) -> ShutdownReport;   // W5
}

pub struct HostRequest<'a> { /* ... */
    /// The machine that performed this operation. With `task`, the whole
    /// identity a handler keys scoped state on.
    pub machine: MachineId,
    /// The task that performed this operation. `None` outside a scheduler
    /// region, which is one identity rather than an absence of one.
    pub task: Option<TaskId>,
    /// The declared footprint of the entry point that reached this operation,
    /// so a handler that can compute its own footprint refuses instead of
    /// acting. **`Option`**, not a bare reference: a caller that declared
    /// nothing passes `None`, and then only the checks the handler can make on
    /// its own apply.
    pub declared: Option<&'a Footprint>,
}
```

The `touched` set is real but it never leaves `ply-host`: `db::check_footprint`
computes it from the statement scan, raises `E0434 DB_FOOTPRINT_UNDECLARED`
against `HostRequest::declared` **before** a connection is acquired, and hands it
to the driver as `Statement::touched`. The machine's own footprint check is the
W1 one — the *resolved* atom against the declared footprint, `E0427`, before
dispatch — and `Machine::host_use` records that resolved atom and no other. So:

- **`E0434` is a preventer and only a preventer.** It fires at prepare time, from
  the scan, and refuses before a row moves. That is strictly better than the
  detector for the case it covers.
- **There is no detector behind it.** A handler that touches an atom its scan did
  not predict is not caught by anything, and neither `HostUse` nor the report
  will ever mention that atom. The class W1 already disclosed — "it cannot catch
  a handler that does more than its registration declared" — is therefore still
  open under W4, and the SQL scan is the whole of the defence rather than the
  first of two. `crates/ply-host/src/db.rs`'s own doc comment on
  `check_footprint` still describes the machine-side detector as though it
  existed; it does not.
- **`E0434` is the program's fault** — attributed and bisected like any other
  program failure, as distinct from `E0427`, which keeps its meaning and stays
  Ply's fault. That much is unchanged.

The preventer: the driver computes the table set at **prepare**
time, once per statement text, and refuses `E0434` from `HostRequest::declared`
before a row moves. The machine's check on `touched` was to have covered the case
the driver's own scan got wrong; it does not exist, so **the scan is unbacked**
and a scan that under-reports is undetected. Neither closes ADR 0008 — a
handler that lies about `touched` is as invisible as one that lies about its
registration — and what the scan alone does close is the case where the *honest*
handler could not tell the truth, which W4's driver is the first handler in the
system to have. The differential test against postgres's own planner, below, is
the only evidence the scan is right, and it is evidence gathered before the run
rather than a check made during one.

**`ply_host::db::scan`** computes the set: a bounded scanner over the statement
text, in Rust, in the TCB, disclosed by `ply hosts`. It recognises `SELECT` /
`INSERT` / `UPDATE` / `DELETE` / `VALUES` / `WITH` and **refuses everything
else** with `E0432` naming the byte offset — never an empty table set, so a
defect is a refusal rather than a footprint that under-reports. Written in Ply
was considered, as ADR 0011 did for HTTP framing, and refused for the one reason
that differs: the driver needs the answer and is Rust, so a Ply copy would be
two scanners, and the disagreement would be between the footprint a test
observes and the footprint the scheduler was given.

**The differential test is the evidence and postgres is the oracle**: over a
generated corpus, `scan`'s set must be a **superset** of the relations
`EXPLAIN (GENERIC_PLAN, FORMAT JSON)` reports. A superset, because the planner
prunes and over-reporting costs concurrency rather than correctness.

**What no scanner can see** — a trigger, a rewrite rule, a cascading referential
action — is asked of the database at bind time. `bind` queries `pg_trigger`,
`pg_rewrite` and `pg_constraint` over every table the program's atoms name, and
an object reaching a table outside the atom it fires under is **`E0438`** before
anything runs. There is no flag to suppress it: a flag that turns a soundness
check off is a flag whose default becomes the one nobody uses.

### The pool

`deadpool-postgres` over `tokio-postgres`, driven by a **current-thread** tokio
runtime on one OS thread owned by `ply_host::db::Reactor`. This resolves ADR 0011's open item — tokio was declared and unused; it is called now. No `Value`
crosses to that thread: the reactor speaks postgres's types and conversion
happens on the machine's thread inside `call` and `poll`. Every `db` operation is
`blocking: true` and answers `Pending`.

| knob | default | bounds |
| --- | --- | --- |
| `--db URL` | — | the database; `sslmode` must be `disable` or `prefer` |
| `--db-pool` | 8 | connections |
| `--db-acquire-ms` | 5000 | waiting for one |
| `--db-statement-ms` | 30000 | server-side `statement_timeout` |
| `--db-idle-txn-ms` | 30000 | server-side `idle_in_transaction_session_timeout` |
| `--db-connect-ms` | 5000 | establishing a connection |
| `--db-statement-cache` | 256 | prepared statements per connection |
| `--db-schema` | — | `<module>.<fn>` returning a `Schema` to verify |

The two server-side timeouts are set with `SET` at every checkout and are not
optional: a statement with no timeout holds a pool slot until the server
restarts and an idle transaction holds locks the rest of the service waits on,
which is ADR 0011's "a bound is part of the contract" applied to a second
protocol.

Acquisition is at `begin` for a transaction and per statement otherwise; a
statement inside a scope reuses its connection and never waits. Release is at
`commit` / `abort`, at `end_entry_point`, and immediately after a scope-less
statement. **Exhaustion is `E0437`, a diagnostic and not a `Failed`** — a
`Failed` is a value a program is invited to swallow, and a swallowed pool
exhaustion is a service returning wrong answers under exactly the load that
produced it. W5 owns backpressure and is where this becomes a shed request.
Connect failure at bind is `E0431`; a database that restarts mid-run is `Failed`
with `08006`, because that is a peer that went away.

**The db reactor is not on `MAX_BLOCKING_OPERATIONS`.** An outstanding query
costs a pending token and no blocking-pool thread, so 64 socket operations and 8
queries can be in flight at once. A parked task leaves the enabled set and
`park` waits for any token; `E0414` covers a state where none can resolve, and
the acquire deadline is what keeps that honest — without it a pool smaller than
the number of open scopes parks every task forever with nothing to read.

**World isolation, bluntly.** Every db-backed test is `Isolation::Host`,
excluded from `isolated: n of m`, never cached, never bisected — all of which
exists already. **W4 does not give a test its own database**: no fork, no
template, no schema-per-test, no truncation. A test's isolation is exactly
footprint conflict grouping over tables plus whatever it does inside a `sandbox`.
Two host-backed tests over disjoint tables therefore run concurrently against
one database, which is correct only if the footprints above are honest — the
sharpest place where they are load-bearing, and why a trigger is refused rather
than warned about. `sandbox`'s limits are stated where it is defined: it does not
isolate DDL, does not roll back a sequence's advance, does not isolate another
connection, and cannot nest past the savepoint bound.

### Statements and parameters

Every data operation takes `(Stmt, List<Param>)` and the driver issues
`Parse` / `Bind` / `Execute`, so parameters cross as typed binary values and are
never part of the statement text. **No function anywhere in W4 interpolates,
escapes or quotes a value into SQL**, and a program cannot express one because
none exists to call.

What that covers exactly: every value. What it does not: a program building
statement text with `++`, because `stmt` takes a `String` and Ply cannot demand a
literal. Two mechanical defences narrow it and neither is a proof — a `;` outside
a string literal or dollar-quoted body is `E0432`, which removes stacked
statements; and the scanner refuses what it cannot account for, so an injected
fragment that changes the statement's shape is usually a refusal. Values are
structurally safe; statement text is the program's own to get right, and W4
makes a dynamic one loud rather than impossible.

| Ply | parameter | result |
| --- | --- | --- |
| `Int` | `int8` | `int2`, `int4`, `int8` |
| `Bool` | `bool` | `bool` |
| `String` | `text` | `text`, `varchar`, `bpchar`, `name`, `uuid` |
| `Bytes` | `bytea` | `bytea` |
| `Float` | `float8` | `float4`, `float8` |
| `Decimal` | `numeric` | `numeric` |
| `Json` | `jsonb` | `json`, `jsonb` |
| `List<a>` | `a[]`, one dimension | `a[]`, one dimension |
| `Option<a>` | `a` or `NULL` | a nullable column of `a` |

Anything outside the table is `E0432` at prepare, naming the postgres type and
the column. At the edges, each a place a driver quietly loses data:

- **`numeric` past scale 28 or 96 bits of mantissa is a decode failure**, never
  a rounding — W2 §4's argument applied to the wire. `NaN` and `±Infinity`
  `numeric` are failures too; substituting zero is the silent-wrong-answer shape.
- An `Int` too large for an `int4` column is `22003` **from the server**, never a
  truncation in the driver.
- **One dimension only.** A multi-dimensional array, or one with a `NULL`
  element, is a decode failure naming the column. `PArray` whose elements are not
  all one non-null constructor is `E0432`; `PArray([])` is legal and takes its
  element type from the parameter description.
- **`Option<Option<a>>` is refused** wherever it appears, as ADR 0011 refuses
  it for `json` and for the same reason: two values, one wire form.
- **No date, time, timestamp or interval type.** A column of one is `E0432`. A
  `timestamptz` is `int8` microseconds and a `date` is `int4` days, in the
  program's own schema, and the value comes from `clock.now()` **as a
  parameter** — better than `now()` in the text, because it puts the
  nondeterminism in the row where `E0412` can see it. `now()`,
  `current_timestamp` and `random()` in statement text are `E0432`. A real gap,
  stated rather than worked around.
- **A duplicate result column name is `E0433`**: a `Row` is a `Map` and
  `select a.id, b.id` would silently keep one.

A statement is prepared per connection, keyed by text, LRU. Preparation is where
the result description arrives, so the scan, the type check, the codec check and
the footprint refusal all happen there — once per statement per connection, never
per execution. `DISCARD ALL` is never issued (it would drop the cache the pool
exists to amortise) and `DEALLOCATE` never (an evicted entry is closed by the
protocol's `Close`). A prepare postgres refuses is **`E0433`** and not a
`Failed`: it is the program's fault, it is the same every time, and it will never
succeed on a retry, so a value would invite a loop on it.

### `derive row` — **not implemented**

`Deriver` has three variants as shipped — `Json`, `Eq`, `Ord`, tags 1, 2, 3 — and
there is no `Row`. `derive row for Item` is a parse error naming the unknown
deriver. What `examples/desk.ply` does instead is written out by hand, with a
comment at its head saying so and naming this section as the specification the
generated code would have satisfied: `RowError`, `RowCodec`, the cell readers and
the two codecs are all Ply source in that file. Everything below is therefore the
design, not the contract, and the reader should not expect to find it in
`crates/ply-derive`.

ADR 0010 named it and ADR 0011 deferred it here "with the `Row` type it is a
codec over".

```ply
pub type RowError = { column: String, expected: String, found: String }
pub type RowCodec<a> = {
  columns: List<String>,
  decode:  (Row) -> Result<a, RowError>,
  params:  (a) -> List<Param>,          // in `columns` order
}
```

`derive row for Item` generates `fn item_row() -> RowCodec<Item>` under ADR 0011's naming, orphan, visibility, expansion-point, hashing and
`E0505`-on-generated-body rules **unchanged**. Everything true of `json` is true
of `row`. What it walks is narrower:

- The target must be a **record**; an ADT is `E0206` naming the type, because a
  row is flat and a sum has no columns.
- A scalar leaf field is a column of that type; `Option<leaf>` is nullable;
  `List<leaf>` is a one-dimensional array; `Option<Option<a>>` is `E0206`.
- **A field that is none of those but is `derivable(json, ·)` is a `jsonb`
  column** through that type's json codec. This is what lets `desk.ply`'s `Order`
  — `lines: List<Line>`, `state: State` — derive at all, and it is where a reader
  should notice W4 has no opinion about normalization: a program wanting
  `order_lines` as its own table writes two codecs and two statements, and
  `derive row` will not do a join for it.
- Anything else is `E0206` naming the field, as `json` reports it.

The column name is the **field name, unchanged** — no case mangling, no prefix.
A rule that guessed would guess wrong once, silently. `columns` is in the record
so the driver can check the result description against the codec at prepare time:
a `select` missing a column the codec needs is `E0433` **before the first row**.

Constraints are `where derivable(row, a)`, checked at the signature per ADR 0011, with the deriver tag added to `tag::CONSTRAINT`'s pinned enumeration.

### The in-memory twin — Ply, and pure

```ply
pub type MemDb = { .. }                      // opaque: tables, sequences, scope stack
pub fn open(s: Schema) -> MemDb
pub fn step(d: MemDb, s: Stmt, ps: List<Param>) -> { db: MemDb, out: Answer }
pub fn begin_step(d: MemDb, level: Isolation, access: Access) -> { db: MemDb, out: Answer }
pub fn commit_step(d: MemDb) -> { db: MemDb, out: Answer }
pub fn abort_step(d: MemDb) -> { db: MemDb, out: Answer }
```

Rows are `Map<String, Cell>` and answers are `Answer`, so the twin and the driver
produce values of one type by construction — ADR 0008's "the same declared
signature", structural rather than promised. A program installs it with an
ordinary `handle` over a region-scoped cell, one clause per `(operation,
resource)`, which is `desk.ply`'s existing shape with the clause bodies changed.
The boilerplate is proportional to tables times operations and it is real; it is
also what makes the discharge visible at the resource granularity the design is
about.

After `with_cell` discharges the cell's atoms a twin-backed test's row is
**empty**: `det`, cached, hermetic without `--host`, and runnable inside
`simulate` — which is what makes a check-then-act race between two requests on
one row findable and replayable from a seed.

**It executes the same `Stmt` text the driver does**, through its own scanner,
because the scanner is where the divergences live and a twin taking a structured
operation would never test it.

Modelled: tables and columns from a `Schema`; rows in insertion order;
`SELECT` with `WHERE` (comparisons, `AND`/`OR`/`NOT`, `IS NULL`, `IN`,
`BETWEEN`, `LIKE`), `ORDER BY` with `ASC`/`DESC` and `NULLS FIRST`/`LAST`,
`LIMIT`/`OFFSET`, `count(*)`; `INSERT … VALUES … [RETURNING]` with
`DEFAULT nextval`; `UPDATE`/`DELETE … [RETURNING]`; `NOT NULL` (`23502`),
`PRIMARY KEY`/`UNIQUE` (`23505`), `FOREIGN KEY` existence with `NO ACTION`
(`23503`) and `CHECK` (`23514`), each naming its constraint; transactions and
savepoints over a stack of snapshots, which a persistent `MemDb` makes a pointer
copy; `22P02` and `22003` type errors; and **the failed-transaction state** —
after a statement fails in a scope, every later statement is `25P02` until the
scope ends or a savepoint below the failure is rolled back to. That last is the
behaviour test doubles omit most often, it is the one that makes a suite pass and
production fail, and it is required.

**Not modelled, and it says so**: anything outside the list answers
`Failed({code: "0A000", …})` — `feature_not_supported`, postgres's own SQLSTATE
— naming the construct. It never guesses and never answers as though it executed,
so a test reaching an unmodelled statement fails loudly, hermetically, in the run
that introduced it. Named so nobody has to discover them: joins, subqueries,
`GROUP BY`, `HAVING`, window functions, CTEs, set operations and every aggregate
but `count(*)`; views, triggers, rules, `ON CONFLICT`, generated columns, partial
and expression indexes; **isolation** — the twin is serial and cannot exhibit a
phantom read, a lost update or a deadlock, so `RepeatableRead` and `Serializable`
behave as serial execution and the law claims nothing about concurrency, which is
the largest thing it does not model and the one a reader most assumes it does;
**collation** — the twin orders `String` by W2's `Value` order, which is byte
order, which is `C`, so any other database collation disagrees on `ORDER BY` over
text; `numeric` past `Decimal`'s range, `float4` rounding, every locale-dependent
function; and **sequences under rollback**, which postgres does not roll back and
neither does the twin, deliberately, because matching the surprising behaviour is
the job.

### The agreement law — `law/host`

An M8 law body's row must be a subset of `{sim.read}` or it is `E0417`, so the
agreement law does not compile as the language stands. Relaxing `E0417` silently
would let a law touch the world without saying so, which is the opposite of every
other decision here, so the relaxation is **declared**, exactly as `test/nondet`
declares a test's:

```ply
law/host "the memory engine agrees with postgres"
  forall (ops: List<Op>) where well_formed(fixture(), ops) {
    replay_memory(fixture(), ops) == replay_live(ops)
  }
```

```rust
pub struct LawDef { /* ... */
    /// `law/host`: the body may carry a non-`{sim.read}` row. Never `proved`,
    /// never cached, `unattempted` without `--host`. In the law's own hash,
    /// written after `tag::LAW` exactly as `TestDef::nondet` is after
    /// `tag::TEST`.
    pub host: bool,
}
```

- The **body** may carry any row. The **guard** may not: a `where` stays pure
  under `E0417` unchanged, because a guard decides the domain and one that could
  act would be choosing which cases to be judged on.
- **Never `proved`, structurally**: the prover's lowering returns "unsupported"
  for a body with a non-empty row, so the certificate cannot be constructed.
  `property` is the ceiling and the tier says so.
- **Never cached**, in either direction, as a host-backed test is not.
- Hermetically — `ply prove`'s default — it is **`W0604`, `unattempted`**, with
  the reason "reaches the host; run `ply prove --host`". Not skipped silently and
  not green: a law about a database that never ran a database, reported as
  passing, is precisely the green-result-over-unexplored-space this project
  audits for.
- A `law` without `/host` whose body carries a non-`{sim.read}` row is `E0417`
  with its message amended to name `law/host` as the fix.

```ply
pub type Which  = Part | Bin
pub type Col    = CSku | CPrice | CTag | CN | CId | CQty
pub type Val    = VText(String) | VNum(Decimal) | VInt(Int) | VNull
pub type Values = PartVals(String, Decimal, Option<String>, Option<Int>)
                | BinVals(Int, Option<String>, Option<Int>)

pub type Op = Insert(Values)
            | Update({ table: Which, column: Col, to: Val, where_col: Col, eq: Val })
            | Delete({ table: Which, where_col: Col, eq: Val })
            | Select({ table: Which, order_by: Col, limit: Int })
            | Count(Which)
            | Begin(Isolation) | Commit | Abort

pub fn render(fx: Schema, op: Op) -> { stmt: Stmt, params: List<Param> }
pub fn well_formed(fx: Schema, ops: List<Op>) -> Bool     // pure guard
```

`Op` is an ordinary ADT, so M8's existing generator and shrinker cover it — no
new generator and no new shrinking rule, which is what makes the law a required
test rather than a project. **Both sides execute the rendered SQL**, so the
twin's scanner is on the tested path; a structured op to the twin and SQL to the
driver would have tested everything except where the bugs are.

**The domain is in the type, and that is the difference between a law and a
decoration.** An earlier draft wrote `table: String`, `order_by: String` and
`values: List<Param>`; M8's generator draws a `String` from the whole of
`String`, so a generated `Count` named the fixture's table with probability zero
and a generated `Insert` was four parameters of the right types in the right
order with probability near it. Measured against the live database: of two
hundred draws under that shape, forty-two survived the guard and **not one of
them contained a statement** — the postgres log for the whole run was four
`BEGIN`s. A green law over that domain is evidence about `render`'s scope
handling and nothing else. So what the guard used to reject is now
unrepresentable, which is a widening of the claim rather than a narrowing of it,
and `well_formed` is left with what a type cannot say: that the scope stack
balances, that a column belongs to the table its operation names, that a value
fits the column it is compared to, and that the table has a primary key.

Comparison is `List<Answer>`: `Rows` in the order returned; `Failed` on **`code`
and `constraint` only**, never `detail`, which is the single most important line
here — a law comparing messages fails on a server upgrade and teaches everyone to
ignore it; and the comparison stops at the first differing index, because a
divergence at op 3 makes 4..n meaningless.

**`render` ends every generated `ORDER BY` with the primary key**, and an
`ORDER BY` alone is not enough. Postgres's bounded sort is a top-N heapsort whose
output among equal keys is neither the heap order nor stable, so twelve rows
sharing one `n` come back `b, c, a` there and `a, b, c` from the twin's stable
insertion sort. Neither is wrong — SQL promises nothing about ties — so a law
that compared them would be refuted by a case nobody could fix, which is worse
than a gap. `well_formed` therefore admits only a table that has a primary key.

**The live side is reset per case, by two ordinary `DELETE`s.** `replay_memory`
builds a fresh `MemDb` per case and postgres does not forget between them, so
without a reset the law is refuted at the second case by rows the first one
committed. A sandbox transaction is refused for it: every generated `Begin` would
become a savepoint, and a nested `begin` at a different isolation level is
`25001`, so the harness's own scope would decide the claim. For the same reason
the fixture's `bin.id` carries no sequence — postgres does not roll back
`nextval` and the twin does not either, so a reset side would carry a position
the twin restarts.

A counterexample prints the shrunk op list **as Ply source a reader can paste
into a test**, the two answers side by side at the first difference, a replay
command that reproduces it exactly, and the tier line saying why `property` is
the ceiling rather than leaving a reader to infer that a green `property` was the
best available.

**The law must be able to fail**, or it is decoration. Two injected divergences
are required tests: a fixture database created with a non-`C` collation must be
refuted on an `ORDER BY` over text, and a twin with the `25P02` state removed
must be refuted — each shrunk to a minimal op list. It lives in
`examples/agreement.ply` with a two-table fixture, **not** in `std.db`, because
`ply test --std` and `ply prove --std` must not need a database to pass.

### Schema, and migrations

**A migration tool is out of scope**: no versions, no up and down, no ordering
across deploys, no diffing a live database into a change script. A **schema is a
value**, which is the part W4 needs, since the twin is built from one and the
law's fixture has to exist.

```ply
pub type ColumnType = TInt | TBool | TText | TBytes | TFloat | TNumeric(Int, Int)
                    | TJson | TArray(ColumnType)
pub type Default    = DNone | DSequence(String) | DLiteral(Param)
pub type Column     = { name: String, ty: ColumnType, nullable: Bool, default: Default }
pub type ForeignKey = { name: String, columns: List<String>,
                        references: String, refers_to: List<String> }
pub type Check      = { name: String, expr: String }
pub type Table      = { name: String, columns: List<Column>,
                        primary_key: List<String>,
                        unique: List<{name: String, columns: List<String>}>,
                        foreign_keys: List<ForeignKey>, checks: List<Check> }
pub type Schema     = { tables: List<Table> }

pub fn create_schema(s: Schema) -> List<Stmt>     // pure: CREATE TABLE text
pub fn drop_schema(s: Schema) -> List<Stmt>
```

Three places a schema has to exist, and how it gets there in each. **The twin**:
`open(schema())`, nothing else involved. **A test or the law's fixture**: the
harness executes `create_schema(schema())` against a database it created — the
only path where the scanner accepts DDL, so a `CREATE TABLE` inside
`db.execute[t]` is `E0432` like any other unrecognised statement. **A production
database**: it already exists, and `--db-schema <module>.<fn>` names a nullary
function returning a `Schema` that the driver materialises at bind time and diffs
against `information_schema` and `pg_constraint`, reporting **`E0435`** for a
missing table, a missing column, a mismatched type, a disagreeing nullability or
a missing constraint, before anything runs.

That third point is most of what a migration tool is bought for — the guarantee
that the code and the database agree, checked at start-up rather than discovered
at the first request — and W4 delivers it without owning the tool that changes
the database. `--db-schema` is optional; without it a mismatch surfaces at
prepare time as `E0433`, later and per statement and still loud.

### `ply hosts`

```
$ ply hosts --host
   9 host handlers · 14 operations · trusted computing base

   OPERATION                 ATOM                  HANDLER                     DET  LINEAR         BLOCKING
   db.begin                  db.write              ply_host::db::begin         no   at-most-once   yes
   db.abort                  db.write              ply_host::db::abort         no   at-most-once   yes
   db.commit                 db.write              ply_host::db::commit        no   at-most-once   yes
   db.execute[items]         db.write[items]       ply_host::db::execute       no   at-most-once   yes
   db.query[items]           db.read[items]        ply_host::db::query         no   at-most-once   yes
   db.query[orders]          db.read[orders]       ply_host::db::query         no   at-most-once   yes
   db.returning[orders]      db.write[orders]      ply_host::db::returning     no   at-most-once   yes
   net.accept[listener]      net.write[listener]   ply_host::tcp::accept       no   at-most-once   yes
   ...

   database
   server     PostgreSQL 18.3 · database desk · collation C · encoding UTF8
   pool       8 connections · acquire 5000ms · statement 30000ms · idle-txn 30000ms
   scanner    ply_host::db::scan · select insert update delete values with
   schema     desk.schema · 2 tables · 11 columns · verified

   digest: b3:7c02e9a41b6d
```

The `database` block exists for the same reason W3's `transport` block does: a
fact the rows cannot carry and a reviewer must not have to derive. The
**collation** is printed because it is the twin's largest silent divergence, and
the **scanner** because it is a parser in the TCB. `db.rollback` does **not**
appear — it is handled in Ply by `transaction` and never reaches the binding; if
it appears, something bound it and that is a defect.

The digest covers the operation rows, the pool numbers, the scanner's accepted
statement set and the schema function's name. It does **not** cover the server
version or the database name, by W3's argument about a certificate fingerprint: a
CI check that broke on a minor server upgrade is one people learn to ignore. Both
are printed and both are in `--json`.

### Amendments to W1

**`--audit-backend` runs a host-backed test once, and counts it unpaired.** A
host handler is not a function: a second machine would charge the card twice,
send the packet twice, insert the row twice. So `execute_directly` runs the plain
machine, asks whether it reached the boundary, and returns without running the
backed one if it did. The run gets no differential audit for that test, which is
correct and must not be silent: the summary carries `audited: n of m · k ran
unpaired` and `--json` carries the same. This is `Isolation::Host` and
`Skipped::Host`'s argument — declare the guarantee inapplicable where it cannot
hold and keep the number honest.

**`end_entry_point` is called on every exit.** The hook is worthless unless it is
called on the diagnostic and budget-exhaustion paths too. `InterpExecutor` calls
it from one place per machine path, beside `arm_footprint_check`, and a required
test asserts an entry point that raised inside a transaction left no open scope
on the connection it used — asserted against `pg_stat_activity`, not against the
driver's own bookkeeping.

### Versions

`RUNTIME_VERSION` to `0.10.0` — `HostAnswer`, `HostReply` and `HostRuntime`
changed shape, the machine checks `touched` and calls `end_entry_point`. Of those
four, two happened: `HostRuntime` gained `end_entry_point(machine)` and the
machine calls it. `HostAnswer` did not change shape, `HostReply` was never
written, and the machine checks no `touched` — see "Footprint granularity" above.
`RUNTIME_VERSION` moved anyway, and correctly: `end_entry_point` alone changes
what a cached `Pass` is a claim about.
`FRONTEND_VERSION` to `0.12.0` — a new deriver (`row`), `LawDef::host`, and the
`law/host` grammar; ADR 0011's rule is that any change to a deriver bumps it.
The deriver did not land, so the bump was paid for `LawDef::host` and the grammar
alone; `Deriver` still enumerates `Json`, `Eq`, `Ord`.
**`BODY_ENCODING` to `7`** — `law_def` writes a host flag after its tag, as
`test_def` writes `nondet`. `PROVER_VERSION` to `0.5.0` — `law/host` is a new
discharge mode with a new ceiling and a new unattempted reason.

`BODY_ENCODING` moving is a one-time cost with a bounded blast radius and the
required test pins the boundary: every law's hash moves once and re-discharges
once, and **no non-law definition's normalized bytes change**, on the whole W3
corpus, asserted byte-for-byte. A milestone that moved definition hashes for a
law's sake would have got the layering wrong.

### Workspace

```toml
tokio-postgres    = { version = "0.7.18", features = ["runtime"] }
postgres-protocol = "0.6.12"
deadpool-postgres = "0.14.1"
tokio             = { version = "1.53.1", features = ["rt", "net", "time", "sync"] }
rust_decimal      = { version = "1.42.1", features = ["db-tokio-postgres"] }
```

`ply-host` gains the first three and finally uses the fourth. `rt` and not
`rt-multi-thread`: one current-thread runtime on one owned OS thread, because
every connection's future is independent, the pool bounds the concurrency, and a
work-stealing runtime would make the thread count a number nobody chose.
`deadpool-postgres` rather than `bb8` or a hand-rolled pool — smallest of the
three, its recycling hook is where rollback-on-release lives, and its size is a
declared number. `postgres-protocol` is taken directly for `numeric` and array
codecs, where `ToSql`/`FromSql` would otherwise need a Rust type per Ply type;
`rust_decimal`'s `db-tokio-postgres` feature supplies the `numeric` conversion
rather than a reimplementation. **No `sqlx`, `diesel` or `sea-orm`**: W4 ships no
query builder and no ORM, and a crate whose whole value is one is a large
dependency for a refused feature. **TLS to postgres is not configured**: `--db`
accepts `sslmode=disable` and `sslmode=prefer` only, anything higher is `E0431`,
and wiring rustls into `tokio-postgres` is a real TCB decision that belongs
beside W5's secrets rather than here as an untested line.

`ply-std` gains `std.db` and no dependency. `ply-eval` gains no dependency.

### New diagnostic codes

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0431 | `DB_NOT_CONFIGURED` | `--host` binds the driver and no `--db` URL was given, or it is malformed, or the server is unreachable at bind time | the run's configuration |
| E0432 | `DB_STATEMENT_REFUSED` | statement text W4 refuses: more than one statement; a construct the scanner cannot account for; a parameter or result type outside the mapping; `now()` / `random()` in the text | the program's |
| E0433 | `DB_PREPARE_FAILED` | postgres refused to prepare, or the result description has a duplicate column name or lacks a column the codec requires | the program's |
| E0434 | `DB_FOOTPRINT_UNDECLARED` | a statement touches a table outside the entry point's declared footprint — at prepare, from the scan against `HostRequest::declared`. The second site, "at answer from `HostReply::touched`", was specified and not built | the program's |
| E0435 | `DB_SCHEMA_MISMATCH` | the live database differs from the `Schema` the run named | the run's configuration |
| E0436 | `DB_TRANSACTION_SCOPE` | a `db` operation from a task that does not own the open scope | the program's |
| E0437 | `DB_POOL_EXHAUSTED` | no connection became available within `--db-acquire-ms` | the run's configuration |
| E0438 | `DB_UNMODELLED_SIDE_EFFECT` | a trigger, rule, or cascading referential action reaching a table outside the atom it fires under | the run's configuration |

E0431, E0435 and E0438 are raised by `bind`, before anything runs, like
E0421–E0423, and **those three are the only ones that join `RESERVED_CODES`**
(`[&str; 18]`). The other five join E0424's row: `Failure::defect` is `false`,
they are attributed like any other failure, and bisection is skipped when the run
reached the host to produce them. They are deliberately *not* reserved, because
each is a refusal the driver is the only component in a position to compute — a
statement's table set, a result description, the task holding a scope, a pool's
occupancy — and reserving them would have `attribute` rewrite the driver's own
diagnosis to `E0502` and send the reader hunting a defect in Ply. `attribute`
still stamps each with the handler path. E0434 is raised from two places, the
driver at prepare time and the machine at answer time, which is why the second
must be the machine's own check rather than a rewrite of a handler's word.

`E0417`'s message is amended to name `law/host`; no other existing code changes
meaning.

### Required tests

The full list is ADR 0011's; these are the ones whose absence would let W4
ship broken rather than merely incomplete.

1. A `db.rollback` deep inside a transaction discards the continuation: the
   statements after it never execute and postgres shows no row.
2. A body that raises propagates the raise, commits nothing, and leaves **no open
   scope** — asserted against `pg_stat_activity`, not the driver's bookkeeping.
3. Nesting is a savepoint: an inner rollback keeps the outer's writes; a nested
   `begin` with a different level is `25001`; a `ReadOnly` write is `25006` from
   the server.
4. A continuation captured before `db.begin` and resumed twice is `E0426` and
   `BEGIN` was issued once.
5. A `db` operation from a task that does not own the open scope is `E0436`.
6. A join across two tables declared with only one is `E0434` **at prepare**,
   before a row is read; declared with both, it runs and records both atoms.
7. `scan`'s table set is a **superset** of `EXPLAIN (GENERIC_PLAN, FORMAT JSON)`'s
   relations over a generated corpus, with no exception; every refused construct
   is `E0432` naming the offset.
8. A trigger, rule or cascading action reaching outside its atom is `E0438` at
   bind time.
9. Two tests over disjoint tables run concurrently against one database; two over
   a shared table with one writer do not.
10. `'; drop table part; --` as a `PText` inserts that string and changes no
    schema; the same bytes in `Stmt::sql` are `E0432`.
11. Every row of the type mapping round-trips both ways; scale-29 `numeric`, a
    `numeric` `NaN`, a 2-D array, a `NULL` array element and a `timestamptz`
    column are each a named refusal and never a coerced value.
12. `derive row for Order` puts `lines` and `state` in `jsonb` and round-trips
    through a real table; an ADT target and a function field are each `E0206`.
13. N executions of one statement issue one `Parse`, by a counting harness
    against the protocol rather than by timing.
14. `--db-pool 1` with two concurrent transactions is `E0437` naming the size —
    not a hang. Both server-side timeouts are set at checkout, read back through
    the same connection.
15. A db operation in flight costs a pending token and **no**
    `MAX_BLOCKING_OPERATIONS` slot.
16. Every modelled clause of the twin has a `det`, hermetic, cached test,
    including `25P02` and the sequence that does not roll back; every unmodelled
    construct answers `0A000` naming it and never a result.
17. `examples/desk.ply`'s suite passes against the twin and against postgres
    **with no source change to any endpoint** — the exit criterion.
18. A check-then-act race between two requests on one row is found by `simulate`
    against the twin and replayed from its seed.
19. The agreement law is `property` with its case count under `--host`, and
    `W0604 unattempted` with a reason without it; a `law/host` is never `proved`
    and the differential audit covers it.
20. **The law finds an injected divergence**: a non-`C` collation is refuted on
    `ORDER BY` over text, and a twin without the `25P02` state is refuted, each
    shrunk to a minimal op list.
21. `create_schema` output produces a database `--db-schema` verifies; a dropped
    column, a changed type and a changed nullability are each `E0435`.
22. Renaming a top-level function selects zero tests and moving a definition
    between modules changes no hash, on a corpus with `db` rows, `derive row` and
    a `law/host`; incremental and `--no-incremental` agree byte-for-byte.
23. `--audit-backend` reports no `E0503`; a host-backed test performs its
    statements **exactly once** across both arms and is reported `host, not
    audited`, with `audited: n of m` excluding it.
24. `E0412` still fires; `ply test` is hermetic without `--host`; an effect-set
    alias and its expansion hash identically over `db` atoms; `Store::open` at
    10,000 definitions stays under 5 ms; `ply prove` reports honest tiers and
    `ply hosts --host` prints the `database` block.
25. **No non-law definition's normalized bytes moved** across the
    `BODY_ENCODING` bump, over the whole W3 corpus.

Plus one `tests/fixtures/` entry per new code.

### Not in W4

Query building and ORMs — a statement is text and a row is a `Map`.
Connection-level `LISTEN`/`NOTIFY`: a notification arrives outside any perform,
which means outside any row, and there is nothing in the effect system for it to
be. Replication, logical decoding and read replicas. `COPY`, in either direction.
Cursors and streaming result sets — `query` materialises, and a result set larger
than memory is a real limit W6 would measure before machinery. Migrations as a
tool. Joins, aggregates and isolation phenomena **in the twin**, and therefore in
the law, whose claim is over sequential operation sequences only. A date, time,
timestamp or interval type. TLS to postgres. Automatic retry of a serialization
failure. **A test database per test** — footprint conflict grouping and `sandbox`
are the whole of the isolation, and they are less than a reader expects.

---

## Operations

`docs/adr/0011-the-web-track.md` has the reasoning; this section is the contract.
**Where it disagrees with any section above, this section wins** — it was
written after them.

### The rule everything else follows from

> A row says what a function records, and a type says where a credential cannot
> go. A log, a configuration and a way to stop are ambient in every other
> language, and ambient is what this one has spent eight milestones removing.

Three consequences decide everything below. **A trace call is a `perform`,
always** — a row cannot be conditional on a flag, so there is no disabled path
that skips it and `--trace off` binds a listed handler rather than an empty
registry. **The environment supplies a value and never causes a binding** — ADR 0011's rule is untouched, and the snapshot is frozen at bind time so that
`config.read` is honestly a read. **Teardown has one pinned order**, because a
drain that commits a half-finished transaction is data loss rather than a mess.

### `std.trace` — Ply source, the declaration

`crates/ply-std/ply/trace.ply`, module `std.trace`, effect `std.trace.trace`.

```ply
pub type Level = Debug | Info | Warn | Error

pub type Field =
  | FInt(Int) | FBool(Bool) | FText(String)
  | FFloat(Float) | FDecimal(Decimal) | FBytes(Bytes)
  | FJson(json::Json)

pub type Fields  = Map<String, Field>
pub type Span    = { id: Int, channel: String }
pub type Outcome = Ok | Failed(String) | Abandoned

pub nondet effect trace {
  write event[c](level: Level, name: String, fields: Fields)   -> Unit
  write enter[c](name: String, fields: Fields)                 -> Span
  write exit[c](span: Span, outcome: Outcome)                  -> Unit
  write count[c](name: String, delta: Int, fields: Fields)     -> Unit
  write gauge[c](name: String, value: Decimal, fields: Fields) -> Unit
  write time[c](name: String, micros: Int, fields: Fields)     -> Unit
}
```

`nondet` is load-bearing and is the sentence `std.net` and `std.db` carry: a
production sink stamps a wall-clock timestamp and mints a span id, so a `det`
test reaching an unhandled `trace` operation is `E0412` **at compile time**,
with `--host` and without it. The twin is what makes such a test compile.

**Six operations on one effect, not two effects.** Ply cannot abstract over two
effects declaring the same operations (the W3 TLS argument), so a separate
`metric` effect doubles every handler clause set in the system to draw a
distinction no scheduling decision consults. The cost is stated: a function that
only increments a counter carries `trace.write[c]`, which reads as "records on
channel `c`", and that is true.

**The resource is a channel and the call site writes it.**
`trace.event[orders](..)` performs `(trace, orders, Write)`. `trace.write` as
one singleton atom is refused: it would serialise every test that records
anything, and it would make a row say "records" and nothing more, which is
`db.write[db]` with a different noun. What it costs: **a channel label cannot be
abstracted over**, so `std.trace` ships **no function that performs** — only
value constructors, the twin, and the sink codec. Every perform is at its call
site with its channel.

`gauge` takes `Decimal` and not `Float`: `Float`'s `==` is not an equivalence
relation and a gauge is a number a test asserts on.

### Spans — implement exactly this

The handler owns the stack, per task. Three of the four ways a computation
leaves a span never run another line of it — a `db.rollback` discards the
continuation, a raise propagates past, an entry point can simply end — so a
program-maintained stack is wrong by construction.

- `enter[c]` pushes onto the performing task's stack; `Span::id` ascends from 1
  **per entry point** and is never reused within one. Per entry point rather
  than per run because an id crosses back into the program — a `Span` is an
  ordinary record so a program can put its id in a field — and a run-global
  counter made a host-backed test's own answer a function of what a
  footprint-disjoint test traced beside it (ADR 0011). Correlating lines
  across entry points is the record's `seq`, which stays run-global.
- `exit[c](s, o)` closes `s` **and every span the same task opened above it**,
  the latter with `Outcome::Abandoned`. Not a warning: a discarded continuation
  is what a rollback *is*, and the `Abandoned` record is the useful signal.
- `exit` naming a span that is closed, never opened, or opened by another task
  **of the same entry point** is **`E0445 SPAN_UNBALANCED`**, naming both tasks
  in the third case. Which of the three it is, is decided from the performing
  entry point's own table: E0445 is attributed and bisected, so its text is what
  a failure report carries, and a classification that consulted the run put
  another test's `MachineId` into this test's report and changed with `--jobs`.
- Whatever is still open when an entry point ends is closed `Abandoned` by
  `end_entry_point` (ADR 0011, doing a second job with no new mechanism)
  and reported **`W0609 SPAN_ABANDONED`** with the count and the innermost name.

### What a span costs when nothing is collecting

There is no configuration under which a trace operation is not performed.
`--trace off` binds `ply_host::trace::discard`, a listed member of the trusted
computing base whose clause answers `Unit`; an empty registry would be `E0424`,
which is correct and is not what "off" should mean.

A disabled span costs exactly: the `Fields` map the call site built, one
`perform`, and nothing else. "Nothing else" is designed rather than observed —
**a call site never formats** (there is no operation taking a rendered message),
**never reads a clock** (the sink stamps `ts`, which is also why a trace call
does not drag `clock.read` into fifty endpoints' rows), and **never allocates a
span record**. **Level filtering happens in the sink**, so `--trace-level warn`
does not make a `Debug` event free — it makes it cost one perform and one map,
and claiming otherwise would need a conditional row.

The number is an exit criterion, not an argument: a benchmark reports the
per-event and per-span cost under `discard`, under `json` and under the twin,
and a counting harness — never a stopwatch — asserts zero clock calls and zero
formatting.

### The trace twin — Ply, and pure

```ply
pub type Kind = KEvent | KEnter | KExit | KCount | KGauge | KTime

pub type Record = {
  seq: Int, kind: Kind, level: Level, channel: String, name: String,
  span: Int, parent: Int, fields: Fields, outcome: Outcome,
  amount: Int, value: Decimal,
}

pub type Sink = { .. }        // opaque: records, the open stack, the next id

pub fn sink() -> Sink
pub fn event_step(s: Sink, c: String, l: Level, n: String, fs: Fields) -> Sink
pub fn enter_step(s: Sink, c: String, n: String, fs: Fields) -> {sink: Sink, span: Span}
pub fn exit_step(s: Sink, sp: Span, o: Outcome) -> {sink: Sink, ok: Bool}
pub fn count_step(s: Sink, c: String, n: String, d: Int, fs: Fields) -> Sink
pub fn gauge_step(s: Sink, c: String, n: String, v: Decimal, fs: Fields) -> Sink
pub fn time_step(s: Sink, c: String, n: String, us: Int, fs: Fields) -> Sink

pub fn drain(s: Sink) -> List<Record>     // closes every open span as Abandoned
pub fn named(rs: List<Record>, n: String) -> List<Record>
pub fn on_channel(rs: List<Record>, c: String) -> List<Record>
pub fn counter_total(rs: List<Record>, n: String) -> Int
pub fn open_spans(s: Sink) -> Int
```

`exit_step` answers `ok: false` where the bound driver answers `E0445`: a twin is
a value and a value does not raise. After a `with_cell` region discharges the
atoms a collecting test's row is **empty** — `det`, cached, hermetic — and it can
assert on the exact records a request produced.

**The `with_cell` belongs inside each test.** One collector around a suite is one
shared cell, which is W4's pooled connection in a new costume.

### `Secret` — the headline

`Secret` is a **builtin type constructor of arity 1**, added to
`BUILTIN_TYPE_CONS` beside `Cell` and `Task`, and

```rust
pub enum Value { /* ... */ Secret(Arc<Value>) }
```

is a **distinct variant**. Both halves are load-bearing. A
`Value::Ctor { name: "Secret", .. }` would be matchable and
`match s { Secret(p) -> p }` would be a one-line escape; `Secret` declares no
constructors, so that pattern is `E0101` at resolution and **there is no pattern
that binds the payload**. A `std`-level record type would be one field access
from useless and a project could declare its own.

```rust
impl Type { pub fn secret(inner: Type) -> Type; }   // Con("Secret", vec![inner])
```

| builtin | type | notes |
| --- | --- | --- |
| `secret_of_string` | `(String) -> Secret<String>` | the only introduction |
| `secret_verify` | `(Secret<String>, String) -> Bool` | constant time over the compared bytes; leaks one bit per call |
| `secret_is_empty` | `(Secret<a>) -> Bool` | one bit, and the check a start-up wants |

There is no `secret_expose`, no `secret_len`, no `secret_map`, no
`secret_concat` and no `secret_slice`, and no `String` in any return type. Each
would have to see the plaintext, and a function that sees it can return it.

Evaluator behaviour, each closing a route that is otherwise total:

- **`Value::render` is `Secret(****)`**, always. This closes the assertion diff,
  the panic payload, `ply run`'s result line, M5's failure JSON, every
  `Diagnostic` that interpolates a value, `--json`, and the result cache.
- **`values_equal` compares two `Secret`s in constant time** and answers a
  `Bool`; a `Secret` is never equal to a non-`Secret`. So `==` and `assert_eq`
  work and neither prints anything.
- **`compare_values` on a `Secret` is `E0502 RUNTIME_ERROR`** naming
  `secret_verify`. Equality leaks one bit per call; an ordering leaks a bit of
  position per call and recovers the value in calls proportional to its length.
  That line is why `derive eq` accepts a `Secret` field and `derive ord` refuses
  one, and the runtime refusal is the backstop for the path that reaches
  `compare_values` without a derivation.
- **`Map<Secret<a>, v>` is `E0206`**, because a `Map` key needs
  `derivable(ord, k)`.
- **The runtime backstop is one gate under `ply_eval::map`, not a check per
  builtin.** Every key entering, leaving or being looked up in a `Map` passes
  through it, and it is the only caller of `insert_mut` in that module. Written
  per call site it was written at four of six: `map_of_entries` and `map_merge`
  reach the tree by another route and were a total ordering oracle over a
  plaintext, through a route ADR 0011 lists as closed. A mitigation spelled
  once per call site is one the next call site does not have.
- **`Secret<a>` is not quantifiable**: a `forall` binder over one is `E0418`,
  exactly as `Cell` and `Task` are. A generator that minted secrets and a
  shrinker that printed counterexamples is a leak by construction.
- **Derivation**: `derivable(json, ·)`, `derivable(ord, ·)` and
  `derivable(row, ·)` are false for `Secret<a>` — `E0206 NOT_DERIVABLE`
  **naming the field** — and `derivable(eq, ·)` holds. Asked twice of one
  predicate, as ADR 0011 specifies: `ply_derive::walk`, and `ply_core`'s walk
  over the *solved* type so `type Password = Secret<String>` is caught too.
- `Value::type_name` is `"Secret"`. `Value::render` truncates nothing, because
  it prints no payload.

The claim, stated so it can be falsified:

> No value of type `Secret<a>` can reach a trace field, a JSON document, a SQL
> parameter, an HTTP response, a diagnostic message, an assertion diff, a panic
> payload, a `--json` object, a definition hash, `frontend.dat` or the result
> cache — because each is reached through a function, a derivation or an
> evaluator path whose parameter type `Secret<a>` does not inhabit.

What it does **not** prevent, and the list is not short: a credential written as
a **source literal** (`secret_of_string("hunter2")` puts `"hunter2"` in a
`Lit::Str`, which normalizes into the definition's bytes and lands in a store
designed never to forget — **the largest hole, and nothing here closes it**); the
plaintext the secret was built from, which is still in scope because Ply is a
value language; `secret_verify` leaking one bit per call, unbounded and
unrate-limited; timing outside the comparison, including a branch that traces on
one arm; a host handler that receives one; **memory** — there is no zeroization,
an `Arc<Value>` is not wiped, and a core dump has the plaintext; and a secret's
*presence*, which is deliberately observable so that a missing credential is
distinguishable from a wrong one.

### `HostOp::secrets`, and `E0439`

```rust
pub struct HostOp {
    /* ... */
    /// Whether this operation may be handed a value containing a `Secret`.
    /// Printed by `ply hosts` in its own column and covered by the digest.
    pub secrets: bool,
}
```

A `perform` whose arguments contain a `Value::Secret`, resolved to a
registration with `secrets: false`, is **`E0439 SECRET_TO_HOST`** before the
handler is called, naming the operation and the argument position. Ply's fault
in the sense `E0427` is: `Status::Panicked`, `Failure::defect` true, not
bisected. **In W5 no operation declares `secrets: true`**, so the column reads
`no` on every row and the check is a tripwire — landed with a user count of
zero, because adding it after the first operation that needed it had already
shipped is the wrong order.

The two credentials W5's own stack holds — the TLS key and the postgres password
— are configured beside the run, never enter the program, and stay in
`ply_cli::db::Secret` unchanged. Two mechanisms, two populations, one promise.

### `std.config` — Ply source, the declaration

```ply
pub nondet effect config {
  read get[k](key: String)    -> Option<String>
  read secret[k](key: String) -> Option<Secret<String>>
}

pub type Shape = SText | SInt | SBool | SSecret
pub type Key = { name: String, shape: Shape, required: Bool, default: Option<String> }
pub type ConfigSpec = { keys: List<Key> }

pub type Values = { plain: Map<String, String>, secret: Map<String, String> }
pub fn values(plain: List<{key: String, value: String}>,
              secret: List<{key: String, value: String}>) -> Values
pub fn get_step(v: Values, key: String)    -> Option<String>
pub fn secret_step(v: Values, key: String) -> Option<Secret<String>>
```

`read`, not `write`, and therefore never a conflict — sound only because the
snapshot is frozen. **There is no `config.set`**; adding one would make the atom
a write and serialise every test in a suite that reads one key. `nondet`, so a
`det` test reaching configuration is `E0412` and must supply it. The resource is
a namespace the call site writes, which buys no scheduling (reads never conflict)
and buys the thing that matters: `ply check --types` says which definitions read
configuration and which read **credentials**.

`get` answers `Option` rather than raising — a missing key is a value the program
matches on, ADR 0011's rule for a second peer. The failure an operator
actually suffers is caught at bind time instead.

**Precedence**, highest first: `--set KEY=VALUE` (repeatable); `--config PATH`
(repeatable, later files win); the process environment, by **exact** key with no
prefix and no mangling; the spec's `default`.

**The file format is `KEY=VALUE`, one per line.** `#` ends a line outside a
value, blank lines are ignored, there is no quoting, interpolation, section or
escape, and the value is the rest of the line with surrounding horizontal
whitespace trimmed. A line without `=`, an empty key, or a key that is not
`[A-Za-z_][A-Za-z0-9_.]*` is **`E0440 CONFIG_UNAVAILABLE`** naming the file and
line. TOML, YAML and JSON are refused because the effect returns
`Option<String>`: a format richer than the type it feeds is a format whose extra
structure is silently dropped, and a parser in a trusted computing base is the
line ADR 0011 says is worth a human's attention.

**The environment is read exactly once**, at bind time, into an immutable
`BTreeMap`, and `std::env::var` is never called at perform time. One line,
carrying three properties: it makes `config.read` honestly a read, which makes
two readers non-conflicting, which makes the conflict graph right; it stops one
test's `setenv` reaching another; and it makes the snapshot a printable fact of
the run.

**Configuration may supply a value and may never cause a binding.** Without
`--host` no source is opened whatever the environment holds, and `config.get` is
`E0424` naming `ply_host::config::get`.

`--config-schema <module>.<fn>` names a nullary function returning a
`ConfigSpec`. At bind time the run materialises it and resolves every key:
a `required` key nothing supplies is **`E0441 CONFIG_MISSING`** naming the key,
its shape and the four places it looked; a value that is not of its `Shape` is
**`E0442 CONFIG_INVALID`** naming the key and the winning source and **never the
value when the shape is `SSecret`**; an **explicit** key — `--set` or `--config`
only, never the environment — the spec does not declare is **`W0607
CONFIG_UNDECLARED`**.

With a spec, `get` on an `SSecret` key answers `None` and `secret` on any other
key answers `None`; without one, both answer whatever the sources hold, so
**§2's containment for configured values is exactly as strong as the spec** and a
run with no `--config-schema` can read a password as a `String`.

**Configuration is read at start-up and is a value thereafter.** No live reload:
a source that can change mid-run is a nondeterminism every reader's row would
have to carry, and two requests in one run seeing different values is a bug class
with no repro. What may be configured is the identity and credentials of the
peers a run talks to and the address it listens on; what may not is anything the
program is *specified* in terms of — `http::Limits`, the route table, business
rules — because those are what tests assert and a value that differs per
environment is a value no test covers. Nothing enforces that line; what W5 offers
is that it is **visible**, since a definition that reads configuration says so in
its row and a `det` test handling no `config` is a proof that what it covered was
the second kind.

### `std.signal` and shutdown

```ply
pub nondet effect signal {
  read stopping()    -> Bool
  read deadline_ms() -> Int    // ms left in the drain; -1 when not draining
}
```

No resource parameter, so the atom is the singleton `signal.read`. `deadline_ms`
exists so a handler can shed rather than begin work it cannot finish.

`ply_host::signal` registers with `tokio::signal` on the reactor thread the db
driver already owns; the handler sets an atomic flag and does nothing else.

| phase | what happens | bound by |
| --- | --- | --- |
| 0 | the flag is set; `signal.stopping()` answers `true` | — |
| 1 | **lead** — accept keeps running so a readiness route can answer `503` | `--drain-lead-ms`, default `0` |
| 2 | **stop accepting** — every `net.accept[s]` answers `0`; listeners close | immediate |
| 3 | **drain** — in-flight connections finish their current request; keep-alive is not offered | `--drain-ms`, default `30000` |
| 4 | **teardown** — the pinned order below | — |
| 5 | **exit** — `0` clean, `3` if the deadline expired | — |

A **second** identical signal exits immediately with `130`/`143` after one line
naming what was abandoned. `SIGTERM` does not exist on Windows; `ctrl_c` is what
binds there and `ply hosts --host` prints which signals the run listens for.

**`examples/desk.ply` drains with no source change.** Its `serve` is a sequential
accept loop that exits on `accept` answering `0`, phase 2 makes it answer `0`,
and its in-flight count at the signal is exactly one — so it cannot lose a
request. That is the exit criterion and it falls straight out of ADR 0011's
decision that `accept` answers `0` rather than raising.

**Teardown order is pinned**, and three of four steps are ordering-sensitive:

1. `end_entry_point`, across drivers in this order:
   1. **db** — every open scope is `ROLLBACK`ed, **never committed**. A commit at
      a deadline commits a half-finished body, and only the body knows whether it
      finished.
   2. **trace** — every open span closed `Abandoned`. After db, so a span can
      record the rollback.
2. **flush the sink** — before the pool closes, so a trace naming a rolled-back
   transaction is written before the connection that rolled it back is gone.
3. **close the pool** — connections closed rather than returned; one whose
   `ROLLBACK` failed is discarded, which is ADR 0011 unchanged.
4. exit.

Any failure in 1–3 is `W0606 HOST_TEARDOWN` naming the driver and what it could
not hand back. It changes no exit code.

```rust
pub trait HostRuntime {
    /* ... */
    /// Whether a stop has been requested. `park` returns when this becomes true
    /// even with no token outstanding — otherwise an idle service never
    /// observes a signal — and the deadlock check consults it so a park that
    /// woke on a stop is not counted as fruitless. Read in exactly two places
    /// in `ply-eval`, and nowhere in inference, a cache key or `Isolation`.
    fn stopping(&self) -> bool;
    /// Called once, after the last entry point, before the process exits. Runs
    /// the pinned order above and answers what it managed. Never called from a
    /// signal handler.
    ///
    /// `drain_ms` is a **bound and not a hint**: a teardown step waiting on a
    /// peer waits at most this long and then discards the connection, because
    /// closing the socket is what aborts the statement.
    fn shutdown(&self, drain_ms: u64) -> ShutdownReport;   // was `deadline:
                                                           // Duration -> Shutdown`
}

/// The type is `ShutdownReport`; four of the six fields below are not the ones
/// that shipped, and the reason each differs is that every field here is a fact
/// the teardown already held rather than a number computed for the banner.
pub struct ShutdownReport {
    /// Transaction scopes rolled back. **Never committed** — a commit at a
    /// deadline commits a half-finished body.
    pub transactions_rolled_back: usize,
    /// Connections closed rather than returned to the pool, **and why**: a
    /// count could not name the connection an operator has to go look at.
    /// (Was `connections_abandoned: usize`.)
    pub connections_closed: Vec<String>,
    pub spans_abandoned: usize,
    /// Records the sink held when it was flushed, and `None` for a run with no
    /// sink bound — which a `0` could not be told apart from.
    /// (Was `events_flushed: u64`.)
    pub records_flushed: Option<usize>,
    /// What the teardown could not hand back, as `W0606` renders it. Empty is a
    /// clean teardown. (Replaces `complete: bool`; `is_clean()` is the boolean,
    /// and it is derived from this rather than reported beside it.)
    pub problems: Vec<String>,
}
impl ShutdownReport { pub fn is_clean(&self) -> bool; }
```

`elapsed_ms` has no counterpart: nothing in the report is timed. The
`"…ms since the signal"` line the operator sees is measured by the run's shutdown
coordinator, which knows when the signal arrived and the report does not.

**A request still running at the deadline is not cancelled.** W5 adds no
cancellation — ADR 0011 deferred it, ADR 0011 argued deadlines sufficed for
sockets, and that is still where it stands. The process tears down and exits, and
the client sees a **connection closed with no response**, or a truncated one if
bytes were written. `--drain-ms` should exceed the program's own
`body_timeout_ms + write_timeout_ms`, which the run cannot check because `Limits`
is a Ply value it never sees; both numbers are in the start-up banner so they can
be compared by eye. A drain that expires is **`W0608 DRAIN_INCOMPLETE`** naming
the connections abandoned and the transactions rolled back, and exits
`EXIT_DRAIN_INCOMPLETE = 3`. A rolled-back transaction is not a lost request in
the dangerous sense — the client got no answer and the database has no partial
write, which a retry fixes; a committed half-transaction is what a retry cannot
fix, and the teardown order is what makes it unreachable.

**`signal` does not bind under `ply test`**, with or without `--host`. A test
that could be ended by the suite's own `ctrl-C`, or that observes a stop another
test requested, is a test whose verdict depends on the terminal. `E0424` names
the twin. This is a deliberate asymmetry with `config`: a frozen read-only
snapshot cannot couple two tests, and a stop flag set once ends every test after
it.

### The three new shared states

W4 found a pooled connection coupling two tests the footprint graph believed were
disjoint. Each of W5's three is a fresh chance to repeat it, so each has an
account rather than a hope:

| state | how it could couple two tests | what W5 does |
| --- | --- | --- |
| the trace sink | one process-wide sink; a test asserting on records sees another's | the atom is `trace.write[c]` — a **write**, per channel — so two tests on one channel are serialised by the existing conflict graph, and a per-test twin discharges the atom entirely. The defect to avoid is one `with_cell` around the suite |
| the config snapshot | a `setenv` seen by another test; one key read twice differing | read **once**, at bind, into an immutable map; `std::env::var` is never called at perform time |
| the stop flag | one stop ends every test after it | `signal` does not bind under `ply test` at all |

**ADR 0008 makes footprint conflict grouping the only isolation a host-backed
test has**, so each is exactly as isolated as its registration's mode and
resource and nothing checks either. Every test reaching any of them is
`Isolation::Host` — counted separately, excluded from `isolated: n of m`, never
cached, never bisected — and W5 adds no case to that machinery.

### `ply build` and the `.plyx` artifact

**W5 ships a whole-program artifact and no incremental transfer.** A deploy must
ship a `ply` binary because the program is interpreted and every guarantee is the
runtime's; the versions move most milestones, so the binary is the part that
actually changes and it is orders of magnitude the larger. Shipping only changed
definitions optimises the small side of a ratio nobody measured — so the required
test prints **both** sizes and the exit criterion carries them, because a
decision of this shape should be re-openable against a measurement. What
incremental transfer would additionally need — a target-side agent, an
authenticated channel, a negotiation, a rollback story, an atomic switch, a
garbage-collection policy — is a product, and a half-built one is worse than
none (ADR 0011's sentence about migrations).

What content addressing *is* worth, and nearly free because the store already
does it, is identity and verification.

```
ply build [PATH] [-o FILE] [--entry NAME] [--config-schema NAME] [--db-schema NAME]
          [--sources] [--digest] [--json]
ply build --diff OLD.plyx [PATH]
ply run FILE.plyx --host [...]
```

An artifact is **the transitive closure of its roots and nothing else**: the
entry point, plus the start-up definitions the build names with
`--config-schema` / `--db-schema`. A schema is a nullary function nothing in
`main` calls, so an entry-point-only closure left it out and the deployed form
lost `E0441 CONFIG_MISSING` and `E0435` — and, since without a spec `config.get`
returns whatever the sources hold, could hand back a credential as an ordinary
`String`. It carries definition bodies in `ply_hash::body`'s encoding keyed by
`DefHash` (the bytes the store already holds, so `ply build` is a copy and not a
second encoder), the namespace needed to resolve them, the entry point's name
and hash, optionally the `SourceMap`, and the header. A start-up root is
resolved by name on the deployed run, so what the artifact owes it is a `NAMES`
entry and a body. **Tests, laws and specs are not in it** — a `test` is a
definition nothing calls, so it is in no root's closure, and that falls out
rather than being filtered. `ply build` prints the roots it shipped, and
`startup none` when there are none; a name resolving to nothing is refused at
build time.

```
header   0  magic        8    b"PLYPROG1"
         8  format       u32  ARTIFACT_FORMAT = 1
        12  flags        u32  bit 0: sources embedded
        16  frontend     32   blake3(FRONTEND_VERSION)
        48  runtime      32   blake3(RUNTIME_VERSION)
        80  body_enc     u32  BODY_ENCODING
        84  std          32   ply_std::digest()
       116  entry        32   the entry point's DefHash
       148  digest       32   below
       180  sections     u32
       184  reserved     u32  0
       188  descriptors  sections × { kind u32, count u32, offset u64, bytes u64 }
```

| kind | section | record | sorted by |
| --- | --- | --- | --- |
| 1 | `BODIES` | `{ hash [32], len u32, bytes }` | hash |
| 2 | `NAMES` | `{ name_off u32, name_len u32, hash [32] }` | name bytes |
| 3 | `STRINGS` | the name blob | — |
| 4 | `SOURCES` | present iff flag bit 0 | path |

A target verifies **everything**, and each check answers a different question:
every body against its own key — `blake3(bytes)`, or
`blake3(component ‖ index_le_u32)` for a member of a component, which
`ply_store::put_body` already refuses on — is **`E0443 ARTIFACT_INVALID`** naming
the hash and offset, so a corrupted transfer is a per-definition refusal rather
than a plausible wrong program; every reference resolving inside the artifact,
also `E0443`; the header's versions against the running binary, which is
**`E0444 ARTIFACT_VERSION`** and its own code because the responses are opposite
(rebuild versus re-transfer), while a differing `ply_std::digest()` is `W0605`
and not an error; and the digest, BLAKE3 domain-tagged `b"ply.program.1"` over
the header from `sections` onward and every section payload in order, rendered
`b3:` plus twelve hex characters by `ply build --digest`.

**Two builds of one source tree produce byte-identical artifacts**, on any
machine, from any directory, from a warm or a cold cache: bodies are normalized,
sections are sorted, nothing carries a timestamp. Reproducible builds falling out
of content addressing rather than being engineered, and a required test run twice
from two roots.

`ply build --diff` is the incremental story delivered as **information** rather
than transport — added, changed, dropped and unchanged definition counts plus the
endpoints reached by a changed one, which is a set difference over two hash sets
and the reverse closure the graph already computes.

What it costs: **a deployed artifact has no spans**, so a production diagnostic
carries `Span::DUMMY` and a synthesized name (`d_<hash12>`, ADR 0003) unless it
was built `--sources` — which puts the source text in whatever receives the
artifact, a disclosure decision, which is why it is a flag, off, and covered by
the digest. **No target-side inventory**: nothing tells a sender what a target
already has, because nothing on the target answers. **No signing**: the digest
establishes identity, not authenticity.

### What an operator sees

**Health and readiness are routes the program writes.** W5 adds no health effect
and no built-in endpoint: a route table is ordinary data, so a framework-supplied
`/healthz` would be a route not in `table()`, and two answers to "what does this
service serve" is how a route and an authorization check come to disagree. What
W5 supplies is the two facts a readiness route cannot otherwise compute, and the
distinction — **liveness** is answerable with an empty row (`desk.ply`'s
`health()` already is, and its `{}` is the proof it cannot be failed by a
database outage), while **readiness** is `!signal.stopping()` and one
`db.query[t](stmt("select 1"), [])`:

```ply
pub fn ready() -> http::Response / {signal.read, db.read[items]}
```

That row is the answer to "what does readiness actually verify", inferred rather
than documented, and a readiness route whose row is `{}` checks nothing and
`ply check --types` says so.

**Structured output**: `ply_host::trace::json` writes one JSON object per line to
**stderr**, and five rules are each a required test. stderr, never stdout,
because every command's `--json` owns stdout and one interleaved line destroys
the document. The program's fields are nested under `fields` **always**, even
when empty, so a field named `level`, `ts` or `span` cannot shadow the envelope.
`ts` is **epoch microseconds**, an integer, not RFC 3339 — Ply has no time type,
a `timestamptz` is already `int8` microseconds by a program's own schema, and a
calendar formatter in a trusted computing base is a dependency and a locale bug
for a field every consumer re-formats; `--trace text` renders `+412.3ms` from the
run's start, which needs no calendar either. **A `Secret` cannot appear**, and
there is deliberately **no redaction pass** — a redaction pass is what W5 is
replacing, and having one would invite someone to rely on it. A line is a single
write, so two tasks cannot interleave one.

```
$ ply run examples/desk.ply --host --db ... --tls ... --config-schema desk.config
   desk.run · ply 0.13.0 · program b3:91af0c33d7e2
   hosts       12 handlers · 19 operations · digest b3:4f19c0a8e2d3
   database    PostgreSQL 18.3 · desk · collation C · pool 8 · schema desk.schema verified
   config      6 keys · 4 environment · 1 --set · 1 default · 2 secrets (values not shown)
   trace       json → stderr · level info · channels db, http, items, orders
   shutdown    signals INT TERM · lead 0ms · drain 30000ms
   listening   0.0.0.0:8137 · tls desk · http/1.1

   ^C
   stopping    drain 30000ms · 1 connection in flight · 0 transactions open
   drained     1 connection · 0 abandoned · 412ms
   database    8 connections closed · 0 rolled back at teardown
   trace       1284 events · 96 spans · 0 abandoned · flushed
   desk.run    exit 0 · served 10429 requests · 4m12s
```

**Nothing is computed for the banner** — every number is a fact the run already
holds. Secret values are absent; secret *keys* and their winning sources are in
`ply hosts --json`, because an operator debugging "it used the wrong credential"
needs to know which source won. A drain that expired prints `W0608` with the
abandoned count and exits `3`, so a rolling restart that loses six requests per
instance cannot report success.

`ply hosts --host` gains a `SECRET` column on every row and three blocks —
`configuration` (sources counted, the schema function, and the keys with values
for non-secret ones and `****` for the rest, each with its winning source),
`observability` (the sink's handler path, the resolved channel list, the span
discipline) and `shutdown` (the signals, the two deadlines, the second-signal
behaviour). The **digest covers** the `SECRET` column, the config schema
function's name and its key names and shapes, the sink path, the channel list and
the shutdown knobs; it does **not** cover resolved config values, the
environment's size or the server version — ADR 0011's rule, that a CI check
which breaks on a deployment's own configuration is one people learn to ignore.

`trace.*` is `Linearity::AtMostOnce` (replaying a continuation across an event
writes it twice, and a duplicated span is a wrong answer about what happened);
`config.*` and `signal.*` are `Repeatable`.

### `ply` flags

```
ply run   [...] --trace <json|text|off>  --trace-level <debug|info|warn|error>
                --config PATH  --set KEY=VALUE  --config-schema <module>.<fn>
                --drain-ms N  --drain-lead-ms N
ply test  [...] --trace <..>  --trace-level <..>  --config PATH  --set KEY=VALUE
                --config-schema <module>.<fn>
ply hosts [...]                                  (the three new blocks)
ply build [PATH] [-o FILE] [--entry NAME] [--sources] [--digest] [--json]
ply build --diff OLD.plyx [PATH]
```

`--trace` defaults to `json` under `ply run` and `off` under `ply test`;
`--trace-level` defaults to `info`. `pub const EXIT_DRAIN_INCOMPLETE: i32 = 3;`
joins `ply-cli`'s exit codes. `ply run --json` emits its object before the entry
point starts and the trace lines follow on stderr.

### New diagnostic codes — landed in `ply_span::codes`

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0439 | `SECRET_TO_HOST` | a host operation registered `secrets: false` was handed a value containing a `Secret` | **Ply's** |
| E0440 | `CONFIG_UNAVAILABLE` | `--config` names an unreadable file, or a line or `--set` that is not `KEY=VALUE` | the run's configuration |
| E0441 | `CONFIG_MISSING` | a `--config-schema` key marked `required` that no source supplies | the run's configuration |
| E0442 | `CONFIG_INVALID` | a resolved value that does not satisfy its declared `Shape` | the run's configuration |
| E0443 | `ARTIFACT_INVALID` | a `.plyx` whose header, digest, section table, body hash or reference closure does not verify | the run's configuration |
| E0444 | `ARTIFACT_VERSION` | a `.plyx` built under a different `FRONTEND_VERSION`, `RUNTIME_VERSION` or `BODY_ENCODING` | the run's configuration |
| E0445 | `SPAN_UNBALANCED` | `trace.exit` naming a span not open on the performing task's stack | the program's |
| W0607 | `CONFIG_UNDECLARED` | a `--set` or `--config` key the schema does not declare | the run's configuration |
| W0608 | `DRAIN_INCOMPLETE` | the drain deadline expired with connections in flight | the run's configuration |
| W0609 | `SPAN_ABANDONED` | spans were still open when an entry point ended | the program's |

E0439 joins `E0427`'s row — the machine's own verdict about the boundary,
`Status::Panicked`, defect, never bisected. E0440–E0442 are raised by
`HostRegistry::bind` before anything runs, like E0421–E0423 and E0431/E0435/E0438.
E0443 and E0444 are raised by the artifact loader before any binding exists.
**Those six join `RESERVED_CODES`, taking it from 18 to 24.** E0445 does **not**:
it is a refusal the trace driver is the only component in a position to compute —
which task holds which span — and reserving it would have `attribute` rewrite the
driver's own diagnosis to `E0502` and send a reader looking for a defect in Ply.
That is ADR 0011's rule unchanged, and E0445 belongs with E0432–E0434, E0436
and E0437. E0445 and W0609 are attributed and bisected like any other program
failure; W0608 and W0606 are run-level and change no verdict.

### Versions

`RUNTIME_VERSION` to `0.11.0` — `Value::Secret`, three builtins, `render` /
`values_equal` / `compare_values` changing on a variant, `HostRuntime::shutdown`
and `stopping`, `HostOp::secrets`, `end_entry_point` closing spans; a cached
`Pass` is a claim about what the evaluator did. `FRONTEND_VERSION` to `0.13.0` —
`Secret` in `BUILTIN_TYPE_CONS`, the four derivability answers, and `E0418` for a
`Secret` binder. **`BODY_ENCODING` stays at `7`** and **`PROVER_VERSION` stays at
`0.5.0`**: no new normalization tag (a `Secret<String>` in a signature is
`Type::Con`, which already encodes by name), and no existing obligation's
discharge can change, because `Secret` is a new type no law could have mentioned.
`ARTIFACT_FORMAT` is `1`. That `BODY_ENCODING` stays is a **required test**, not
an observation: the whole W4 corpus normalizes byte-for-byte identically, and the
front-end cache is discarded on the `FRONTEND_VERSION` bump while the **result
cache is untouched**, so no test re-runs for a reason other than a source edit.

**Two constants have moved since, and no Versions block above records them.**
Read `ply_store`'s own doc comments, which do:

| constant | as shipped | what moved it after W5 |
| --- | --- | --- |
| `RUNTIME_VERSION` | `0.11.2` | `0.11.1`: `map_of_entries` / `map_merge` refuse a `Secret` key rather than ordering it, and a span id is minted per entry point rather than per run. `0.11.2`: a nullary pure definition is memoized — no value moves, but the calls pending under a second reference to one do (see "The constant memo") |
| `FRONTEND_VERSION` | `0.15.0` | `0.14.0`: a handler clause for a polymorphic operation is universally quantified, so a clause answering a concrete type for an operation declared `-> a` is `E0201` where it was accepted. `0.15.0`: ADR 0017's region surface — `with_region[r]`, the escape check on resolved types, and a variant field declared as a concrete `Cell` becoming `E0446` |
| `PROVER_VERSION` | `0.5.0` | unchanged since W4 |
| `BODY_ENCODING` | `7` | unchanged since W4 |
| `FRONTEND_FORMAT` | `5` | — |

Each of those last two `FRONTEND_VERSION` bumps is a front end that *refuses* a
program the previous one accepted, which is the case the constant's own comment
says nothing else catches.

### Workspace

```toml
tokio = { version = "1.53.1", features = ["rt", "net", "time", "sync", "signal"] }
```

**One feature, and no new crate.** Rejected, each with a reason: **signal-hook**
— tokio already owns a current-thread reactor on one OS thread, and a second
signal mechanism in a trusted computing base is two things that can both claim
`SIGTERM`. **tracing / tracing-subscriber / opentelemetry** — the sink is fifty
lines of JSON writing, the effect is the interface, and a subscriber ecosystem's
whole value is the ambient dispatch this milestone exists to remove; adopting one
means two notions of a span, one of which is in no row. **toml / serde_yaml /
figment** — the format is `KEY=VALUE` because the effect returns
`Option<String>`. **chrono / time / jiff** — `ts` is epoch microseconds, two
lines from `SystemTime`. **zeroize** — it would zero one `Arc<str>` while the
evaluator copies values freely, which is a promise the runtime cannot keep and a
badge it should not wear.

`ply-std` gains `std.trace`, `std.config` and `std.signal` and no dependency;
`ply-host` gains `trace.rs`, `config.rs`, `signal.rs`; `ply-cli` gains
`commands/build.rs` and `artifact.rs`; `ply-eval` gains a `Value` variant, three
builtins and two `HostRuntime` methods and **no dependency**.

### Changes to `examples/desk.ply`

`effect log` is **deleted** and its call sites become `trace.event[orders]` and
`trace.event[items]`; `effect set Desk` loses `log.write` and gains
`trace.write[orders]` and `trace.write[items]`. That moves the hashes of every
definition annotated with `Desk` and everything reaching them, which is correct —
the signature changed and selection is exact about it — and a required test
asserts the definitions *not* reaching `trace` are untouched. A `ready()` route
with the row `{signal.read, db.read[items]}` joins `health()` with the row `{}`.
An API-key check on `POST /orders` reads `config.secret[credentials]` at start-up
and compares with `secret_verify` — the only end-to-end `Secret<a>` in the
repository. `run_memory` gains six `trace`, two `config` and one `signal` clause
over three region-scoped cells and stays hermetic: its row is still
`{net.write[conn], net.write[listener]}`. **Not changed**: `serve`,
`serve_connection`, the framing, the route table, or any endpoint's behaviour.

### Required tests

The full list is ADR 0011's; these are the ones whose absence would let W5
ship broken rather than merely incomplete.

1. Two channels do **not** conflict in the concurrency graph and two definitions
   on one channel do; `ply check --types` prints `trace.write[orders]` with no
   flag.
2. A `det` test reaching an unhandled `trace` operation is `E0412` at compile
   time, with `--host` and without it; a twin-backed tracing test's row is empty,
   it is `det`, it is cached, and its second run is a hit.
3. A `db.rollback` inside a span leaves it `Abandoned` in `drain`'s output with
   every earlier record intact; a raise does the same and reports `W0609`.
4. `trace.exit` on a span that is closed, never opened, or opened by another task
   is `E0445`, naming both tasks in the third case.
5. **The cost property**, by a counting harness and never a stopwatch: N events
   under `discard` perform exactly N host operations, allocate exactly N `Fields`
   maps, call `clock.now()` zero times and format zero strings — and the same for
   `Debug` events under `--trace-level warn`. A published benchmark reports the
   per-event and per-span cost under `discard`, `json` and the twin.
6. Every route in the containment table is a compile error, one test each:
   `Secret ++ String`, a `Secret` in a `Field`, in a `Param`, in
   `bytes_of_string`, in `panic`, as a `Map` key, and `derive json` / `row` /
   `ord` over a record holding one (`E0206` **naming the field**) while
   `derive eq` succeeds.
7. `match s { Secret(x) -> x }` is `E0101`; a failing `assert_eq` over a record
   holding a `Secret` prints `Secret(****)` on both sides, and the same bytes
   appear in `--json`, in the cached failure report and in `--explain`.
8. **End to end on `desk.ply`**: a right key is accepted and a wrong one refused,
   and the run's stderr, its `--json`, its `.ply-cache` directory and its
   `frontend.dat` are searched for the credential's bytes and it appears in
   **none** of them.
9. `forall (s: Secret<String>)` is `E0418`; `compare_values` on a `Secret` is
   `E0502` naming `secret_verify`; `secret_verify` shows no monotone step count
   over mismatches at increasing positions.
10. A host operation registered `secrets: false` handed a `Secret` is `E0439`,
    `Status::Panicked`, not bisected; the `SECRET` column changing alone moves
    the `ply hosts` digest.
11. Precedence: one key from all four sources resolves to `--set`, and removing
    sources in order walks it down to the default.
12. The environment is read **once**: a `setenv` between two `config.get` calls
    in one run does not change the second's answer. Two tests reading
    configuration are in one concurrency group and run concurrently.
13. `E0440` for an unreadable file, a line without `=`, an empty key and a
    non-identifier key, each naming file and line; `E0441` naming the key and the
    four sources; `E0442` for a bad `SInt`, and for a bad `SSecret` **without
    printing the value**; `W0607` for a `--set` key the schema does not declare
    and **not** for an environment key.
14. `config.get` on an `SSecret` key answers `None` and `config.secret` on a
    non-secret key answers `None`; a handler-supplied configuration is `det`,
    cached and hermetic, and without it `E0424` names
    `ply_host::config::get`.
15. **`desk.ply` drains with no source change**: a signal mid-request lets it
    complete, `accept` answers `0`, `serve` returns, the listener closes, exit
    `0`.
16. A transaction open at the deadline is **rolled back and never committed**,
    asserted against `pg_stat_activity` and the table's contents rather than the
    driver's bookkeeping; a trace record naming the rollback is written before
    the pool closes.
17. A second signal exits `130`/`143` immediately; an expired drain is `W0608`
    with the abandoned count and exit `3`; a completed one is exit `0`.
18. An **idle** service with no traffic and no outstanding token observes a
    signal and exits — `park` returns on `stopping()` and `E0414` is not
    reported. `--drain-lead-ms 2000` keeps accepting for two seconds while
    `signal.stopping()` already answers `true`.
19. `signal.stopping()` under `ply test --host` is `E0424` naming the twin, and
    works under `ply run --host`.
20. `ply build` twice from two different absolute roots, one cold cache and one
    warm, produces **byte-identical** artifacts and the same digest; running the
    artifact serves byte-identical responses over the full route table.
21. A flipped bit, a truncation and a dangling reference are each `E0443` naming
    the definition; a different `BODY_ENCODING` is `E0444` and not `E0443`; a
    differing `ply_std::digest()` is `W0605` and the run proceeds.
22. An artifact contains no `test`, no `law` and no span other than
    `Span::DUMMY`; `--sources` changes the digest; `--diff`'s counts agree with
    `ply hash` over the two trees; the artifact's and the binary's sizes are both
    printed.
23. A trace line goes to stderr and `ply run --json`'s stdout document parses
    with trace output interleaved; a program field named `level`, `ts`, `span` or
    `channel` appears under `fields` and shadows nothing; `--trace off` binds
    `ply_host::trace::discard` and `ply hosts` lists it, while an **empty**
    registry is `E0424`.
24. The start-up banner's every number matches the corresponding
    `ply hosts --json` field; the digest moves for the `SECRET` column, a config
    key name or shape, the sink path, the channel list and a shutdown knob, and
    does not move for a resolved config value, the environment's size or the
    server version.
25. Renaming a top-level function selects zero tests and moving a definition
    between modules changes no hash, on a corpus with `trace`, `config`, `signal`
    and `Secret` rows; incremental and `--no-incremental` agree byte-for-byte;
    `E0412` still fires; `ply test` is hermetic without `--host`; bisection names
    the correct culprit and `--audit-backend` reports no `E0503` with a `Secret`
    round-tripping identically; a seeded race is found against
    the twin **with tracing installed** and replayed exactly; an alias containing
    `trace` and `config` atoms hashes as its expansion; `Store::open` at 10,000
    definitions stays under 5 ms; `ply prove` reports honest tiers and
    `ply hosts` prints the three new blocks.
26. **No definition's normalized bytes moved** across the W5 change, over the
    whole W4 corpus — `BODY_ENCODING` stays at `7` and this is what proves it.

Plus one `tests/fixtures/` entry per new code.

### Not in W5

Metrics backends — no Prometheus, OTLP, StatsD or push gateway; `count`, `gauge`
and `time` are records in a sink and a time series is a consumer's job. Log
shipping — no rotation, syslog, network sink or batching; one JSON object per
line on stderr and the supervisor owns the rest. Orchestration and autoscaling —
no image, manifest, service discovery, rolling-restart controller or replica
count. Distributed tracing propagation — W3 has no HTTP client, so there is no
outbound context, and an inbound `traceparent` is a header, which is data.
Sampling — the sink drops by level and nothing else, because a policy that
silently discards is what this project audits for. **Cancellation**, still, which
is what a request at the drain deadline costs and is the largest gap in the
milestone for the second time. Live configuration reload. Incremental deploy
transport, with the measurement that would re-open it. Artifact signing.
Zeroization and every memory-level guarantee about a `Secret`. A `Secret` that
survives concatenation, transformation or partial disclosure — no `secret_map`,
`secret_concat` or `secret_slice`, because each would have to see the plaintext.
**Rate limiting, backpressure and load shedding**: ADR 0011 said W5 owns
backpressure and W5 does not — `E0437 DB_POOL_EXHAUSTED` is still a diagnostic
rather than a shed request, and turning it into a `503` needs a policy about
which requests to refuse and a way to refuse one without ending the run. That is
a promise W4 made that W5 is breaking, stated rather than quietly dropped.

---

## Test isolation under regions

ADR 0017 The forkable world is gone, so the exemption that rested on it is
gone with it. `ply-test`'s scheduler and its report are what change.

### `ply-test::schedule` — landed

```rust
/// Effects whose atoms name a region label. Exactly one at v0: the builtin
/// `cell`. **Not an exemption** — it is how a report says *why* two tests
/// contend, which is a fact a reader can act on by renaming one.
pub const REGION_SCOPED: &[&str] = &["cell"];
/// Effects whose atoms name an input no test can write. Exactly one: `sim`.
/// The only exemption left, because a seed is handed to a test rather than
/// shared between two, and no memory model changes that.
pub const AMBIENT: &[&str] = &["sim"];

pub fn is_region_scoped(atom: &EffectAtom) -> bool;
pub fn contends(atom: &EffectAtom) -> bool;          // !is_ambient
pub fn region_isolated(f: &Footprint) -> bool;       // was `world_isolated`
pub fn shared_footprint(f: &Footprint) -> Footprint; // now drops ambient only
/// Shared, and only over region labels: isolated under the forkable world and
/// coloured now. Exactly what ADR 0017 costs.
pub fn contends_only_over_regions(f: &Footprint) -> bool;

pub enum Isolation { Region, Shared }                // was `World | Shared`
```

`group_by_conflict` is unchanged in shape and colours `shared_footprint`s. What
changed is the projection: a `cell` atom conflicts like any other atom, because
a region closed at the end of a test is not a fork — two tests writing one label
write one piece of state, and the group's fixture is mutated in place.

**Meaning does not move.** A test's allocations still cannot be observed by
another test; the colouring is what pays for it now instead of the fork.

### The region a group runs in — `ply-test::region`

```rust
// Every signature below was typed in `World` before R2 removed it: `build` took
// `FnOnce() -> World`, `open` answered `World`, `close` took `&World`.
pub struct GroupRegion { /* fixture: Fixture */ }

impl GroupRegion {
    pub fn empty() -> GroupRegion;
    /// Runs the seed once, on the caller's thread. A worker lives for exactly
    /// one concurrency group, so "once per worker" is ADR 0017's "once per
    /// group".
    pub fn build(seed: impl FnOnce(&mut TaskRegions) -> Value) -> GroupRegion;
    /// The boundary: below it is the group's, at or above it is the test's.
    pub fn mark(&self) -> usize;
    pub fn open(&self) -> (TaskRegions, Value);
    /// Discards what the test allocated; keeps what it wrote to the fixture.
    /// Answers whether anything was reclaimed.
    pub fn close(&mut self, after: &Arena) -> bool;
    pub fn fixture(&self) -> &Fixture;
}
```

`InterpExecutor::with_fixture` builds one per worker; `Worker::region` exposes
it. A searched test opens the region per interleaving and writes nothing back:
a replayed test would otherwise apply its writes once per schedule, and which
world the group ended with would depend on which interleaving ran last.

In-place mutation is sound **because** of the colouring above: a group's members
have pairwise non-conflicting footprints and a region label now conflicts, so no
two tests in a group can name one piece of fixture state and the order they run
in cannot decide a verdict.

### Reporting — the ADR 0008 trap, again

A report that still says `world` after the world is gone is a lie, and a count
that stopped being true is worse than one that was never printed. So:

- `--explain` says `isolation: region` or `isolation: shared {atoms}`, and
  appends `(region labels)` when every atom that made a test shared is one.
- `Parallelism` gains `region_contended`: shared tests that contend *only* over
  a region label. Printed on every run under `--explain`, and in `--json`.
- `report::SCHEMA_VERSION` is **4**. `tests[].isolation` says `region`, and it
  is a narrower claim than `world` was.

### Required properties

1. Two tests naming one region label are coloured apart; two tests naming
   different labels are one group. The cost is the collisions, never the count.
2. A group of tests on distinct labels runs concurrently and none of them
   observes another's cells, over repeated rounds, under `--audit-backend`.
3. After every test, the world its worker holds carries that test's writes and
   nothing else, and the group's region is back at its mark.
4. The group's fixture is built once, and test *k* opens the region on test
   *k−1*'s write.
5. Verdicts, groups and `Parallelism` are identical at `--jobs 1` and
   `--jobs 8`.
6. Adding *N* region-isolated tests changes the group count by zero.
7. `open` is **linear** in the fixture's size — it replays the fixture's slots
   into a fresh arena, where `World::fork` cloned one pointer at ~1 ns. That is a
   real regression against ADR 0005 and it is the price of dropping the
   persistent world; state it rather than discovering it. What must still hold is
   the weaker claim: one test's whole region cost stays under rebuilding the
   fixture for it, at every size.

## The compiled seam's second effect fact — `DefInfo::internally_effectful`

Landed 2026-08-24, closing `CONTRIBUTING.md` §"Things known to be broken" item
11. Recorded here because it is a change to a public signature and nothing else
in the tree reads this file.

```rust
pub struct DefInfo { /* ... */
    /// Whether running this definition can execute a `perform` that
    /// `footprint` does not show. Transitive: true when this body is written
    /// with `perform` or `handle`, or when anything reachable from it is.
    pub internally_effectful: bool,
}
```

**Why a row could not carry it.** A `Footprint` is the set of atoms that
*escape* a call. An atom performed inside a call and discharged by a `handle`
inside that same call escapes nothing, so both `footprint` and `performed` are
empty and *correct* — row inference subtracts exactly what a handler discharges,
which is what makes `performed` a subset of `footprint` in the first place. The
seam needed the other question, "can an atom be performed and discharged in
here", and no row can express it because discharging is what takes an atom out
of a row.

**Why it is transitive.** A definition that merely calls one that discharges its
own effects publishes an empty row too and is written with neither keyword, so a
per-body bit clears it and loses the same atoms. `fn wrapper(x) = handled(x)`
publishes `{}` for both rows and records `state.read` when it runs.

**Not on `KnownDef` and not on `CachedDef`, deliberately** — the one place this
change does *not* follow the precedent `performed` set, so the reasoning is
written down rather than left to be inferred. `Checker::mark_internal_effects`
recomputes it every run from the parsed program, and gate 2's path
(`publish_known`) is a parsed file whose AST is right there, so a stored copy
would be a second answer to a question the AST already answers. Gate 1's path
has no AST at all, and `driver.rs`'s `restore_skipped` therefore seeds `true`:
by gate 1's own import rule — a module imported by a parsed module is forced to
parse, to a fixpoint — nothing a run can call is restored that way, so the
conservative value is never read and costs nothing. **`FRONTEND_VERSION`,
`FRONTEND_FORMAT` and `BODY_ENCODING` do not move**, because no stored bytes
changed.

**Polarity is part of the contract.** Every `DefInfo` is constructed with the
flag set; `mark_internal_effects` is the only thing that clears it, and only for
a definition it positively cleared. "Nothing walked this" and "do not enter
this" are one answer, which is `compiled.rs`'s stated default that declining is
what everything not positively cleared gets.

**No command prints it, and that is a gap rather than a decision defended.**
`footprint` and `performed` are both reviewable — `ply check --types
--explain` prints the declared row, the body's inferred row and the difference
— and this fact is not. A reader who wants to know why a definition is not
being entered has to run the checker in a test. It was left out because adding
a field to that output moves bytes several tests pin, and item 11's fix was
already reaching across three crates; the cost of the omission is that the
seam's second gate is the only one whose input a reviewer cannot see from the
command line.

### Required properties

1. A definition that performs and discharges its own operations publishes an
   empty `footprint` *and* an empty `performed`, and is refused by
   `Gate::InternalEffects` rather than by `Gate::PublishedRow` — the two are
   separate variants so that neither gate's test can be satisfied by the other.
2. The refusal follows a call chain to a fixpoint, not one hop: through four
   wrappers, through a mutually recursive pair entered at either member, and
   through a call reached only from a lambda bound in a `let`.
3. A genuinely pure chain of the same depth is still admitted. A gate that
   refused everything with a call in it would satisfy 1 and 2.
4. Deleting the gate fails at corpus scale, not only in unit tests:
   `tests/fixtures/self_handled_effect.ply` under
   `crates/ply-eval-tests/tests/suite/differential_corpus.rs`.

**It over-approximates, by how much is measured, and the direction is stated.**
An edge is any reference that denotes a definition of this program, minus the
enclosing definition's own parameters — which shadow a global of the same name
for the whole body, so a bare reference to one never denotes it. Locals bound
further in are **not** resolved away, so a lambda parameter or a `let` binder
that shadows a definition's name still contributes that definition's edge. The
error is always "refuse a definition that could have been entered" and never the
reverse.

Its size on this tree, `examples/` being the largest corpus: of 1,067
definitions, 953 publish an empty row, and `Gate::InternalEffects` refuses
**11** of those. Without the parameter subtraction it refuses 29, and the extra
eighteen are one shape — `desk.item_named(shelf, ..)` folds over its own
parameter and `desk` also declares `fn shelf`. The eleven are real:
`desk.under` is `handle { .. } with { signal.stopping() -> false }` under an
empty published row, and the other ten reach it.

**What the gate costs the seam today: nothing measurable.** With the
nested-machine backend of `differential_corpus.rs` attached over every corpus in the tree
except the new fixture, the counters read **18,772 entered / 101,567 declined
over 1,011 tests** with the gate and **18,772 / 101,567** without it — the same
numbers, because every call those eleven definitions make in this corpus is
already refused by `Gate::ArgumentShape`, which precedes both effect gates. That
is also the honest reading of item 11's *"latent rather than live"*: on this
tree what stops those eleven is the argument shape, not — as the entry said —
that the only backend refuses `handle` at compile time.

> **Narrowed 2026-08-31, when the argument test became a type test.** *"every
> call those eleven definitions make in this corpus is already refused by
> `Gate::ArgumentShape`"* now holds for one of its three reasons rather than
> three. A `Record` and a `List` cross this seam — `Gate::ArgumentType` decides
> them from the declared parameter type — and what still refuses `desk.under`
> is its **closure** parameter, `body: () -> a / {Serving | e}`, on the value's
> discriminant with no lookup. Re-measured over `examples/` either side of the
> widening: `Gate::InternalEffects` goes **54 → 91** refusals and
> `Gate::PublishedRow` **385 → 1,144**. More calls reach both effect gates and
> both refuse every one; nothing that discharges its own effects became
> enterable, and
> `a_definition_that_discharges_its_own_effects_is_in_the_corpus_and_is_never_entered`
> is still what says so.

## W4 — record update

### `ply-syntax` — landed

```
record       := "{" [ ".." path "," ] { field "," } "}"
             |  "{" ".." path "}"
path         := ident { "." ident }
```

`..` is the token the record *pattern* already uses (`TokenKind::DotDot`); `...`
is a parse error naming the correct spelling. A `{` followed by `..` is a record
literal, because `..` cannot begin a statement — one token of lookahead, and
`{x}` is still a block. A second `..`, or one not in first position, is a parse
error.

```rust
pub enum ExprKind {
    // ...
    /// `{..base, f: e}` — **parse-time only**. Rewritten into `ExprKind::Record`
    /// before `parse_module` returns.
    RecordUpdate { base: Box<Expr>, fields: Vec<(Ident, Expr)> },
}
```

**No crate downstream of `ply-syntax` can observe this variant**, and that is
checked rather than argued: `crates/ply-syntax/src/tests.rs
no_record_update_survives_parse_module_anywhere_in_the_tree` parses every `.ply`
file in the repository plus a file that uses the syntax, through both
`Module`-returning entry points, and asserts none survives. `ply-hash`,
`ply-core`, `ply-eval` and `ply-prove` carry arms for it that refuse — three
`unreachable!` and one `Blocker::UnexpandedSugar` — and those arms are safe only
because of that guard.

> **The prover's blocker was `Blocker::Region`.** That line read "three
> `unreachable!` and one `Blocker::Region`". A record update is not a `perform`,
> a `handle` or a `simulate`, and `ply_corpus::discharge::label` prints a
> blocker's words to a reader, so the arm now carries its own variant.
> Unreachable today, so nothing can print it and no test watches it fire; the
> half that *is* checked is that `label` matches `Blocker` exhaustively with no
> wildcard — delete the new row and `ply-corpus` fails to build with `E0004`.

- **Expansion happens inside `ply_syntax::parse_module`**, immediately after
  `effect_set::expand`, for the same reasons and with the same shape.
  `parse_expr` runs it with an empty context, so a spread in a bare expression
  refuses (`E0116`) rather than leaking.
- **The canonical expansion**: copies first, sorted by field name, valued
  `base.<name>`; then the written fields in the order written. Sorted because
  `reordering_the_fields_of_a_record_type_is_free` is an invariant the suite
  asserts; written-last was chosen when field position decided an append's
  cost, and is kept because the expansion participates in definition hashes
  (position decides nothing since ADR 0034). Sorted **by name and not by length** is a
  separate claim and is pinned separately, because a suite written in
  one-character field names orders identically under either comparator and says
  nothing. Each of `crates/ply-syntax/src/tests.rs
  copies_are_sorted_by_name_and_not_by_length`,
  `record_update_hashes_as_its_expansion` and
  `a_projected_base_hashes_as_its_expansion` therefore carries **two**
  mixed-length pairs, one each way: a pair whose longer name sorts first
  (`ab`/`b`, `aa`/`z`, `pp`/`q`) and a pair whose shorter one does (`a`/`bb`,
  `a`/`zz`, `p`/`qq`).

  > **One pair was not enough, and the gap had the same shape as the defect it was
  > written to close.** The sentence above ended:
  >
  > > : `crates/ply-syntax/src/tests.rs
  > > copies_are_sorted_by_name_and_not_by_length` reads the emitted order off the
  > > tree, and `record_update_hashes_as_its_expansion` and
  > > `a_projected_base_hashes_as_its_expansion` both carry field names whose
  > > lexicographic and length orders disagree.
  >
  > They did — but all three pairs disagreed in the **same direction**. `ab`, `aa`
  > and `pp` each sort before their partner lexicographically *and* are longer, so
  > the three cases rule out a shortest-first comparator and leave a longest-first
  > one green. Shown rather than argued: replacing the comparator with
  > `b.len().cmp(&a.len()).then(a.cmp(b))` makes the expander emit
  > `(rec (bb (field s bb)) (a (field s a)) (c 1))` — a wrong order — and left
  > `ply-syntax` 229/229, the hash audit 53/53, `ply-cli --test stdlib` 25/25,
  > `--test incremental` 25/25, `--test modules_hash_audit` 2/2,
  > `ply-core --test record_update` 8/8 and `ply-eval --test equivalence_audit`
  > 38/38 **all green**. The mirror pairs close it: with them, both length
  > directions go red.
- **The base is a path**, `s` or `state.limits`, never a call. The expansion
  writes `base.f` once per copied field, so a base that could perform or allocate
  would run once per field.
- **The shape is read from this module's own `type` items and this file's
  written annotations, and nothing else** — the ADR 0011 restriction,
  for the ADR 0002 gate-1 reason. A shadowing binder *removes* the annotation
  rather than inheriting it.

### `ply-hash` — landed

**Nothing.** No new tag, no encoding change, **no `FRONTEND_VERSION` and no
`RUNTIME_VERSION` bump**. The sugar is gone before `ply-hash` sees anything, and
that is the whole design: `{..s, a: 1}` and `{b: s.b, c: s.c, a: 1}` are one
definition with one `DefHash`
(`crates/ply-hash-tests/tests/suite/audit.rs record_update_hashes_as_its_expansion`).

### `ply-core` — landed

**No typing rule.** By the time inference runs the update *is* a record literal,
so the update's type is the base's type because the expansion emits the base's
field set, and the width meets the same exact-key-set unification
(`crates/ply-core/src/unify.rs`) every record literal meets. A too-wide shape is
`E0101` from `ExprKind::Field`; a too-narrow one is `E0201` wherever the result
meets a known record type — which is **not total** for a `{..s}` no annotation
ever constrains, and is marked so in ADR 0023 rather than claimed.

### New diagnostic codes — landed

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0116 | `RECORD_UPDATE_SHAPE` | the base of `{..b, ..}` has no record shape this file can name: an unannotated or shadowed binder, a type declared in another module, a qualified base, a generic alias, a sum type, or an alias chain over sixteen deep | the program's |
| E0117 | `RECORD_UPDATE_FIELD` | a named field is not a field of the base — an update replaces, it does not widen | the program's |

Fixtures: `tests/fixtures/record_update_shape.ply`,
`tests/fixtures/record_update_field.ply`, checked by
`crates/ply-core-tests/tests/suite/record_update.rs
the_fixtures_produce_the_codes_they_are_named_for`.

### `std.http` — landed

`chunk_trailers` writes `{..state.limits, max_header_bytes:
state.limits.max_trailer_bytes}`. **Every limit it does not deliberately replace
is copied from the limit of the same name**, which is asserted on the parsed tree
at `crates/ply-cli/tests/suite/stdlib.rs
chunk_trailers_copies_every_limit_it_does_not_replace` — a test that goes red on
a mispairing while `ply check` stays green, because all thirteen `Limits` fields
are `Int`. The same assertion covers the three converted test helpers
(`the_limits_helpers_vary_only_the_bounds_they_are_named_for`), and
`limits_with_pairs_each_bound_with_the_parameter_named_after_it` closes the one
mispairing conversion does not remove: `limits_with` writes seven bounds from
seven `Int` parameters, so `max_chunk_size: chunk_line` still type-checks and
only the naming convention stands between it and a wrong bound.

`limits_with`, `limits_keeping` and `limits_streaming` are record updates over a
`let base: Limits = default_limits();` lift as well, each varying only the bounds
its name promises. **`default_limits` is the one site that still spells `Limits`
out, and it cannot stop**: it constructs from nothing, so there is no base to
update.

The moved set is the transitive-dependent set of the four converted definitions,
exactly — "moved but not a dependent: none", "dependent that did not move: none":

| corpus | entries | moved |
| --- | --- | --- |
| `crates/ply-std/ply/http.ply` | 206 = 150 definitions + 56 tests | **47** = 23 definitions + 24 tests |
| `examples/` | 1,428 = 1,067 definitions + 361 tests | **91**, of which 44 are `desk.*` and 47 are `std.http`'s own entries |
| the other seven shipped modules | — | **0** |

`examples/desk.ply` imports `std.http`, so `desk` is a transitive dependent and
its 44 moving is the intended behaviour. Nothing else written under `examples/`
moved. No `FRONTEND_VERSION` or `RUNTIME_VERSION` bump: a binary built from this
branch's compiler with the **base** `http.ply` embedded hashes the base corpus to
**0 moved**, so the whole table is attributable to the `.ply` edit.

> **The three helpers were on the deferred list, and both numbers beside them
> were misleading.** This read:
>
> > `default_limits`, `limits_with`, `limits_keeping` and `limits_streaming`
> > still spell `Limits` out. They were left alone so that no `DefHash` outside
> > `chunk_trailers`' dependency cone moved: 40 of 206 entries in `std.http`,
> > **84 of 1,428 in `examples/` — all 84 inside that cone** — and 0 in the
> > other seven shipped modules.
>
> The deferral was deliberate and it was spent as soon as the criterion it
> protected was established. It had a cost while it lasted: crossing
> `max_chunk_size` and `max_chunk_line` inside `limits_keeping` left `ply check`
> reporting `checked 2 modules, 150 definitions, 56 tests` and every targeted
> suite green, because all thirteen `Limits` fields are `Int` and nothing
> exercised those two bounds through that helper.
>
> `84 of 1,428` was a correct reading of the tree as it then stood
> (`chunk_trailers` alone) and is superseded by the 91 above, not withdrawn.
> **`40 of 206` was a scope, not a total**: it is 20 definitions and 20 tests,
> and "40" alone reads as "40 definitions". A neighbouring reading of "60 moved
> (40 definitions + 20 tests)" is not a contradiction and not a second
> measurement — it comes from hashing the whole `crates/ply-std/ply/` directory,
> where `http.ply` appears **twice**, once as the module `http` (the file) and
> once as `std.http` (the copy compiled into the binary, reached through another
> module's `import`), and then keying the comparison on the bare name, which
> double-counts the definitions and collapses the tests that collide. One file,
> one honest denominator: `ply hash` on `http.ply` itself.

> **The `examples/` figure was wrong, and it was wrong in the reassuring
> direction.** This sentence read:
>
> > ... 40 of 206 entries in `std.http`, 0 of 1,428 in `examples/`, 0 in the
> > other seven shipped modules.
>
> The **claim** the figure supports still holds; the **figure** did not. Nothing
> outside `chunk_trailers`' dependency cone moved, but 84 entries inside it did,
> and 19 of them are `desk.*` definitions in `examples/` rather than `std.http`
> ones — `examples/desk.ply` imports `std.http`, so it is a transitive dependent
> and moving is what content addressing is *for*.
>
> **A `0` here is what a stale binary reports**, which is why it is worth naming
> the trap rather than just the number. `crates/ply-std/src/lib.rs` `include_str!`s
> every `crates/ply-std/ply/*.ply` **into the binary**, so `import std.http`
> resolves to the copy compiled in, never to the file on disk. Reverting or
> editing `http.ply` and re-running `ply hash` **without rebuilding** therefore
> changes nothing, and reports `0 moved`. Worse, the instrument check this
> repository prescribes cannot see it: `find crates -name '*.rs' -newer
> target/release/ply` prints nothing, because the stale input is a `.ply`.
> **Add `-name '*.ply'` to that check before trusting any hash-movement number.**
>
> Re-taken with a binary verified fresh against both `*.rs` and `*.ply`, on
> corpora copied out of each checkout with `.ply-cache` excluded (a stale cache in
> a checkout will also skew this): `examples/` **1,428 entries, 84 moved**,
> identical across two runs; the moved set and the transitive-dependent set of
> `std.http.chunk_trailers` are **equal**, with "moved but not a dependent: none"
> and "dependent that did not move: none". `std.http` re-reads **40 of 206**, as
> above, and the whole `crates/ply-std/ply/` tree moves nothing outside
> `http` — so the exit criterion holds; only the count was misreported.
>
> *(Round 2: "as above" in that last sentence points at the paragraph as it stood
> then — `chunk_trailers` alone, 40 of 206. The paragraph now reads 47 of 206,
> because three more definitions were converted afterwards.)*

## W4 — the `?` operator

Full argument: `docs/adr/0028-the-question-mark-operator.md`. User-facing
surface: GUIDE §6.10.

### `ply-syntax` — landed

```
postfix      := primary { "(" args ")" | "." ident | "." op resource args | "?" }
```

`?` is a new token (`TokenKind::Question`) in the **tightest** precedence tier,
beside `f(x)` and `r.field`, so `f(x)?.g` is `(f(x)?).g`, `-x?` is `-(x?)` and
`a == b?` is `a == (b?)`. Ply has no ternary, so nothing else can claim the
token, and **no `.ply` file in the tree contained a `?` outside a string or a
comment** before this change — the lexical addition changes no existing file's
token stream.

```rust
pub enum ExprKind {
    // ...
    Try { operand: Box<Expr> },
}
```

**Parse-time only, exactly as `RecordUpdate` is.** `parse_module` runs
`effect_set::expand`, then `record_update::expand`, then `try_op::expand`, and an
unexpanded `ExprKind::Try` never escapes. `parse_expr` runs `expand_bare`, where
every `?` refuses because there is no enclosing `fn` to read a return type off.

**The pass order is load-bearing.** `record_update` reads written `let x: T`
annotations to find a base's field list; a `?` expanded first would already have
turned `let x: T = e?;` into an `Ok(x)` arm binder, which `bind_pattern` binds
untyped, and a `{..x, f: 1}` after it would become a spurious `E0116`.

The canonical expansion, **failure arm first**:

```text
region  C[e?]              =>  match e { Err(er) -> Err(er), Ok(x) -> C[x] }
                               match e { None    -> None,    Some(x) -> C[x] }
{ s..; let p = e?; rest }  =>  { s..; match e { Err(er) -> Err(er),
                                                Ok(p)   -> { rest } } }
```

- **Failure first is measured, not chosen.** `normalize.rs` writes arms in source
  order, so this parameter decides whether converting the corpus moves 129 hashes
  or zero. The corpus writes the failure arm first **129 times to 3** for
  `Result` and **11 to 6** for `Option`. One rule for both.
- **The `let`-pattern case is required, not a convenience.** The general rule
  would emit `Ok(t) -> { let p = t; rest }`, a different definition with a
  different hash.
- **Synthesized binders are `?0`, `?1`, …** — a `?` cannot occur in an
  identifier, which is what `ModuleName::qualify` relies on for the same reason.
  A binder named `t` would capture a `t` the author wrote in the expression it
  wraps. Names are erased by de Bruijn levelling, so the counter costs no hash.
- **The mode is the enclosing `fn`'s written return type**, followed through this
  module's own `type` aliases to a head constructor, and no further — the ADR
  0002 gate-1 restriction `record_update` and `effect set` both take.
- **Position:** from the region root down to the `?`, every step unconditional
  and everything evaluated before it `ply_syntax::ast::is_pure`. That predicate
  **moved here from `ply-hash`'s `normalize.rs`** so that `?`'s safety rule and
  `commutable_run`'s reordering rule are one implementation. The move was free:
  `crates/ply-hash-tests/tests/suite/map.rs a_map_body_normalizes_to_a_pinned_hash` is
  unmoved.

### `ply-hash` — landed

**Nothing.** No new tag, no encoding change, **no `FRONTEND_VERSION` and no
`RUNTIME_VERSION` bump**. `e?` and the `match` it stands for are one definition
with one `DefHash` (`crates/ply-hash-tests/tests/suite/audit.rs try_hashes_as_its_longhand`,
which carries a reversed longhand and an `assert_ne!` so the pair cannot pass
vacuously).

ADR 0023's mixed-length field names have **no analogue here**: they exist because
record-update copies are *sorted*, and match arms are not. What is pinned instead
is that the emitted order is the corpus's and that reversing it is visible.

### `ply-eval` and the machine — landed

**Nothing.** Both evaluators have evaluated `Match` since W1, so `--audit-backend`
compares two engines running a tree a human could have typed. There is no new
node to disagree about, no unwind, no frame kind, and nothing to get wrong at a
`handle` boundary. `interp.rs` and `code.rs` carry `unreachable!` arms; the
defensive walks (`code.rs::barrier_binders`, `region_kind.rs`,
`differential.rs`, `engine.rs`, `retarget.rs`, `prove/context.rs`) carry
one-line recursion; `ply-prove`'s `lower.rs` records `Blocker::UnexpandedSugar`,
sharing `RecordUpdate`'s, because a prover's safe answer is never a term it
guessed.

**Effect rows are untouched by construction.** The pass emits a `match` and two
constructor applications, all pure, so the row of `C[e?]` is the row of the
longhand. There is no row rule for `?` because there is no `?` after the parser
(`crates/ply-core-tests/tests/suite/try_op.rs a_try_adds_nothing_to_the_row`).

### New diagnostic codes — landed

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0118 | `TRY_SCOPE` | the enclosing `fn` — or lambda, which may write `-> T` before a block body — has no return type this file can read as `Result` or `Option`: no `->`, another head, a type parameter, a generic alias, a cross-module name, a `test`/`law`/spec, a lambda without one, a `handle` clause or body, a `with_cell`/`with_region`/`simulate`, or a module that declares — or imports unqualified — its own `Ok`/`Err`/`Some`/`None` | the program's |
| E0119 | `TRY_POSITION` | the `?`'s early exit would change what runs — something conditional between it and the region root, or something impure evaluated before it — or would discard a written `let` annotation | the program's |

**No third code and no typing rule.** By the time inference runs, `e?` *is* the
`match`, so a `Result<_, E1>` bound in a `-> Result<_, E2>` function is an
ordinary `E0201`; `?` performs **no error conversion**, there being no `From` in
Ply, and the eight corpus sites that map their error keep their `match`.

Fixtures: `tests/fixtures/try_scope.ply`, `tests/fixtures/try_position.ply`,
checked by `crates/ply-core-tests/tests/suite/try_op.rs
the_fixtures_produce_the_codes_they_are_named_for`.

### The conversion — landed

139 sites: `db.ply` 128 (119 `Result` + 9 `Option`), `json.ply` 7, `http.ply` 1,
`config.ply` 1, `router.ply` 1, `desk.ply` 1.

| corpus | entries | moved |
| --- | ---: | ---: |
| `crates/ply-std/ply/` | 941 definitions + 270 tests | **0** |
| `examples/` | 1,067 definitions + 362 tests | **0** |

Taken twice, byte-identical; binary verified fresh by
`.github/binary-is-current.sh`, which covers `.rs`, `.ply` and dep-info — the
`.rs`-only shape of that check is what the record-update entry above records
being caught by. `ply test --audit-backend` is `0 failed, 176 passed` over
`crates/ply-std/ply` and `0 failed, 186 passed` over `examples`, on both sides.

**Zero moved is a claim about the gate as much as about the change.** With the
two arms emitted in the other order the same conversion moves **392 of 1,211**
entries in `crates/ply-std/ply/`.

**One site refused and was reverted.** `examples/desk.ply`'s `decoded` writes the
canonical shape *inside a `fold` lambda*, and `?` refused a lambda (`E0118`) until
lambdas could write a return type. The
design phase recorded "0 of the 129 sites sit under a lambda"; it is **1**, and
that is a measured cost of the restriction rather than a hole in it.

**`ply-derive` emits no `?`, deliberately.** `crates/ply-derive/src/emit.rs`
keeps emitting `json::decode_and_then(...)` and hand-written `Err(de) -> Err(de)`
matches, so `generated_form_audit.rs` and `derivation_determinism_audit.rs` keep
their pinned text verbatim. Putting `?` into generated code *would* be a
`FRONTEND_VERSION` bump — gate 1 keys on raw file content, so a file whose bytes
did not change would reuse a stale generated definition. `std.json`'s
`decode_and_then` and `decode_map` therefore stay `pub` and unconverted.
