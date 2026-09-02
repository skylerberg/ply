//! The stored form of a definition body: DESIGN.md §3's `Definition`, the one element of `Hash ->
//! (Definition, Type, Footprint)` the store never held.

use indexmap::IndexMap;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::*;
use std::collections::{BTreeMap, BTreeSet};

use crate::DefHash;
use crate::normalize::{binop_byte, mode_byte, tag, unop_byte};

/// The generation of the encoding below.
pub const BODY_ENCODING: u32 = 7;

/// `Decimal`'s bounds, which are the type's rather than a policy: a sign, a 96-bit mantissa and a
/// scale of `0..=28`.
const MAX_DECIMAL_SCALE: u32 = 28;
const MAX_DECIMAL_MANTISSA: u128 = (1u128 << 96) - 1;

/// A definition that is its own strongly connected component: the payload is its normalized bytes
/// and `blake3(payload)` is the key.
const KIND_SOLO: u8 = 0;
/// A member of a mutually recursive component: the payload is the *component's* bytes — every
/// member, so the cycle can be rebuilt — and the key is `blake3(blake3(payload) ‖ class_le_u32)`.
const KIND_MEMBER: u8 = 1;

fn local_name(level: u32) -> String {
    format!("_l{level}")
}

fn ty_param_name(level: u32) -> String {
    format!("_t{level}")
}

fn row_param_name(level: u32) -> String {
    format!("_e{level}")
}

fn ident(name: impl Into<Symbol>) -> Ident {
    Ident {
        name: name.into(),
        span: Span::DUMMY,
    }
}

/// Sixteen hex characters rather than the full sixty-four: long enough that a collision across a
/// project's definitions is not a thing that happens, short enough that a diagnostic about a
/// reconstructed program is readable.
fn short_name(prefix: char, hash: DefHash) -> Symbol {
    Symbol::new(format!("{prefix}{}", &hash.to_hex()[..16]))
}

/// A member of a component, given the component's own hash.
pub(crate) fn member_hash(component: DefHash, class: u32) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&component.0);
    hasher.update(&class.to_le_bytes());
    DefHash(*hasher.finalize().as_bytes())
}

/// One definition's canonical body bytes, in the envelope that makes them self-checking against the
/// [`DefHash`] they are filed under.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StoredBody(Vec<u8>);

enum Shape<'a> {
    Solo(&'a [u8]),
    Member { class: u32, payload: &'a [u8] },
}

impl StoredBody {
    pub(crate) fn solo(encoding: &[u8]) -> StoredBody {
        let mut out = Vec::with_capacity(encoding.len() + 1);
        out.push(KIND_SOLO);
        out.extend_from_slice(encoding);
        StoredBody(out)
    }

    pub(crate) fn member(component: &[u8], class: u32) -> StoredBody {
        let mut out = Vec::with_capacity(component.len() + 5);
        out.push(KIND_MEMBER);
        out.extend_from_slice(&class.to_le_bytes());
        out.extend_from_slice(component);
        StoredBody(out)
    }

    /// Bytes read back from a store.
    pub fn from_bytes(bytes: Vec<u8>) -> Option<StoredBody> {
        let body = StoredBody(bytes);
        body.shape()?;
        Some(body)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn shape(&self) -> Option<Shape<'_>> {
        let (kind, rest) = self.0.split_first()?;
        match *kind {
            KIND_SOLO => Some(Shape::Solo(rest)),
            KIND_MEMBER => {
                let index = rest.get(..4)?;
                let class = u32::from_le_bytes(index.try_into().ok()?);
                Some(Shape::Member {
                    class,
                    payload: &rest[4..],
                })
            }
            _ => None,
        }
    }

    /// The one `DefHash` these bytes may be filed under.
    pub fn key(&self) -> Option<DefHash> {
        match self.shape()? {
            Shape::Solo(bytes) => Some(DefHash::of(bytes)),
            Shape::Member { class, payload } => Some(member_hash(DefHash::of(payload), class)),
        }
    }

    pub fn verify(&self, hash: DefHash) -> bool {
        self.key() == Some(hash)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BodySet {
    defs: IndexMap<DefHash, StoredBody>,
    /// Parallel to [`crate::HashOutput::tests`].
    tests: Vec<StoredBody>,
}

impl BodySet {
    pub fn insert(&mut self, hash: DefHash, body: StoredBody) {
        self.defs.insert(hash, body);
    }

    pub fn push_test(&mut self, body: StoredBody) {
        self.tests.push(body);
    }

    pub fn get(&self, hash: DefHash) -> Option<&StoredBody> {
        self.defs.get(&hash)
    }

    pub fn contains(&self, hash: DefHash) -> bool {
        self.defs.contains_key(&hash)
    }

    pub fn defs(&self) -> impl Iterator<Item = (DefHash, &StoredBody)> {
        self.defs.iter().map(|(h, b)| (*h, b))
    }

    pub fn tests(&self) -> &[StoredBody] {
        &self.tests
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty() && self.tests.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Fn,
    Type,
    Effect,
}

#[derive(Debug)]
pub struct Reconstruction {
    pub program: Program,
    /// Hash -> the program-wide name this program declares it under.
    pub names: IndexMap<DefHash, Symbol>,
    pub kinds: IndexMap<DefHash, ItemKind>,
    /// Parallel to [`BodySet::tests`]: `<module>.<label>`, the key a test's result is cached under.
    pub test_keys: Vec<Symbol>,
}

impl Reconstruction {
    pub fn name_of(&self, hash: DefHash) -> Option<&Symbol> {
        self.names.get(&hash)
    }

    pub fn kind_of(&self, hash: DefHash) -> Option<ItemKind> {
        self.kinds.get(&hash).copied()
    }
}

/// The module a reconstructed program's tests live in.
const TEST_MODULE: &str = "ply_tests";

fn corrupt(message: impl Into<String>) -> Diagnostic {
    Diagnostic::warning(codes::CACHE_CORRUPT, message)
        .note("the stored definition body is not one this build can decode")
        .note("run `ply cache clear` to discard it and recheck from source")
}

fn incomplete(missing: &BTreeSet<DefHash>) -> Diagnostic {
    let names: Vec<String> = missing.iter().take(8).map(|h| h.short()).collect();
    Diagnostic::warning(
        codes::CACHE_UNREADABLE,
        format!(
            "{} definition {} referenced by a stored body {} not stored",
            missing.len(),
            if missing.len() == 1 { "body" } else { "bodies" },
            if missing.len() == 1 { "is" } else { "are" },
        ),
    )
    .note(format!("missing: {}", names.join(", ")))
    .note("reconstruct the whole closure of a definition, not one definition of it")
}

struct Unit {
    id: DefHash,
    members: Vec<Vec<u8>>,
    hashes: Vec<DefHash>,
    names: Vec<Symbol>,
    module: ModuleName,
    binder: Symbol,
}

/// A program-wide name per hash — `store.orders.place` — which is what a deployable artifact
/// carries beside the bodies.
pub type Namespace = BTreeMap<DefHash, Symbol>;

struct Layout {
    units: Vec<Unit>,
    /// Hash -> (unit, class).
    by_hash: BTreeMap<DefHash, (usize, usize)>,
}

/// What a unit is called, once every unit is known.
fn naming(units: &[Unit], namespace: &Namespace) -> Option<Vec<(ModuleName, Vec<Symbol>)>> {
    let mut out: Vec<(ModuleName, Vec<Symbol>)> = Vec::with_capacity(units.len());
    // A qualifier is a module's last segment, so two modules sharing one cannot both be referred to
    // from a third — the same ambiguity `import` has in source, and not one worth inventing a
    // disambiguation for here.
    let mut binders: BTreeMap<Symbol, ModuleName> = BTreeMap::new();
    let mut taken: BTreeSet<Symbol> = BTreeSet::new();

    for unit in units {
        let mut module: Option<ModuleName> = None;
        let mut simple = Vec::with_capacity(unit.hashes.len());
        for hash in &unit.hashes {
            let qualified = namespace.get(hash)?;
            let (prefix, name) = qualified.as_str().rsplit_once('.')?;
            let owner = ModuleName::from_dotted(prefix);
            // A component's members are mutually recursive, so they were declared in one module;
            // anything else did not come from a program.
            if *module.get_or_insert_with(|| owner.clone()) != owner {
                return None;
            }
            if !taken.insert(qualified.clone()) {
                return None;
            }
            simple.push(Symbol::new(name));
        }
        let module = module?;
        if binders
            .entry(module.default_binder())
            .or_insert_with(|| module.clone())
            != &module
        {
            return None;
        }
        out.push((module, simple));
    }
    Some(out)
}

/// Unpacks the blob a component is hashed from: a count, then each member's encoding
/// length-prefixed, in ascending byte order.
fn unpack(payload: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut cursor = Cursor::new(payload);
    let count = cursor.u32().ok()?;
    let mut members = Vec::new();
    for _ in 0..count {
        let len = cursor.u32().ok()? as usize;
        members.push(cursor.bytes(len).ok()?.to_vec());
    }
    // Class order, not byte order: a member's position *is* its class, which is what lets a decoded
    // intra-component reference name a member.
    let mut distinct: Vec<&[u8]> = members.iter().map(Vec::as_slice).collect();
    distinct.sort_unstable();
    distinct.dedup();
    if !cursor.done() || distinct.len() != members.len() {
        return None;
    }
    Some(members)
}

impl Layout {
    fn build(bodies: &BodySet, namespace: &Namespace) -> Result<Layout, Vec<Diagnostic>> {
        let mut units: IndexMap<DefHash, Unit> = IndexMap::new();
        let mut diags = Vec::new();

        for (hash, body) in bodies.defs() {
            let Some(shape) = body.shape() else {
                diags.push(corrupt(format!(
                    "the body stored for `{hash}` is not a definition body"
                )));
                continue;
            };
            let (id, members) = match shape {
                Shape::Solo(bytes) => (DefHash::of(bytes), vec![bytes.to_vec()]),
                Shape::Member { class, payload } => {
                    let Some(members) = unpack(payload) else {
                        diags.push(corrupt(format!(
                            "the component body stored for `{hash}` is malformed"
                        )));
                        continue;
                    };
                    if class as usize >= members.len() {
                        diags.push(corrupt(format!(
                            "the body stored for `{hash}` names member {class} of a component with \
                             {} of them",
                            members.len()
                        )));
                        continue;
                    }
                    (DefHash::of(payload), members)
                }
            };
            if !body.verify(hash) {
                diags.push(corrupt(format!(
                    "the body stored for `{hash}` hashes to `{}`",
                    body.key().map_or_else(|| "nothing".into(), |h| h.short())
                )));
                continue;
            }
            units.entry(id).or_insert_with(|| {
                let hashes: Vec<DefHash> = match members.len() {
                    1 if matches!(shape, Shape::Solo(_)) => vec![id],
                    n => (0..n as u32).map(|i| member_hash(id, i)).collect(),
                };
                Unit {
                    id,
                    names: hashes.iter().map(|h| short_name('d', *h)).collect(),
                    hashes,
                    members,
                    module: ModuleName::from_dotted(short_name('m', id).as_str()),
                    binder: short_name('m', id),
                }
            });
        }

        if !diags.is_empty() {
            return Err(diags);
        }

        // Module order decides nothing — resolution keys on names — but a reconstruction that is
        // not byte-identical run to run is not one an artifact can be diffed against.
        let mut units: Vec<Unit> = units.into_values().collect();
        units.sort_by(|a, b| a.id.cmp(&b.id));

        if let Some(naming) = naming(&units, namespace) {
            for (unit, (module, names)) in units.iter_mut().zip(naming) {
                unit.binder = module.default_binder();
                unit.module = module;
                unit.names = names;
            }
        }

        let mut by_hash: BTreeMap<DefHash, (usize, usize)> = BTreeMap::new();
        let mut names: BTreeMap<Symbol, DefHash> = BTreeMap::new();
        for (u, unit) in units.iter().enumerate() {
            for (class, hash) in unit.hashes.iter().enumerate() {
                by_hash.insert(*hash, (u, class));
                let qualified = unit.module.qualify(&unit.names[class]);
                if let Some(other) = names.insert(qualified.clone(), *hash)
                    && other != *hash
                {
                    diags.push(corrupt(format!(
                        "definitions `{hash}` and `{other}` both reconstruct as `{qualified}`"
                    )));
                }
            }
        }
        if !diags.is_empty() {
            return Err(diags);
        }
        Ok(Layout { units, by_hash })
    }
}

/// Rebuilds a checkable, evaluable program from stored bodies.
pub fn reconstruct(bodies: &BodySet) -> Result<Reconstruction, Vec<Diagnostic>> {
    reconstruct_relinked(bodies, &BTreeMap::new())
}

/// [`reconstruct`], under the names the definitions were built with rather than under synthesized
/// ones.
pub fn reconstruct_named(
    bodies: &BodySet,
    namespace: &Namespace,
) -> Result<Reconstruction, Vec<Diagnostic>> {
    reconstruct_with(bodies, &BTreeMap::new(), namespace)
}

/// [`reconstruct`], with every stored reference rewritten through `relink` before it is resolved.
pub fn reconstruct_relinked(
    bodies: &BodySet,
    relink: &BTreeMap<DefHash, DefHash>,
) -> Result<Reconstruction, Vec<Diagnostic>> {
    reconstruct_with(bodies, relink, &Namespace::new())
}

fn reconstruct_with(
    bodies: &BodySet,
    relink: &BTreeMap<DefHash, DefHash>,
    namespace: &Namespace,
) -> Result<Reconstruction, Vec<Diagnostic>> {
    let layout = Layout::build(bodies, namespace)?;
    let mut missing: BTreeSet<DefHash> = BTreeSet::new();
    let mut diags: Vec<Diagnostic> = Vec::new();
    // Keyed by name rather than pushed per unit: with a namespace restored, two units may belong to
    // one module, and two `Module`s of one name is not a program.
    let mut modules: IndexMap<ModuleName, Module> = IndexMap::new();
    let mut names: IndexMap<DefHash, Symbol> = IndexMap::new();
    let mut kinds: IndexMap<DefHash, ItemKind> = IndexMap::new();

    for (u, unit) in layout.units.iter().enumerate() {
        let mut items = Vec::with_capacity(unit.members.len());
        let mut imports: BTreeSet<ModuleName> = BTreeSet::new();
        let mut slots: BTreeMap<DefHash, u32> = BTreeMap::new();

        for (class, encoding) in unit.members.iter().enumerate() {
            let mut decoder = Decoder {
                c: Cursor::new(encoding),
                layout: &layout,
                unit: u,
                values: 0,
                ty_params: 0,
                row_params: 0,
                imports: &mut imports,
                slots: &mut slots,
                missing: &mut missing,
                relink,
            };
            match decoder.item(unit.names[class].clone()) {
                Ok((item, kind)) => {
                    names.insert(unit.hashes[class], unit.module.qualify(&unit.names[class]));
                    kinds.insert(unit.hashes[class], kind);
                    items.push(item);
                }
                Err(bad) => diags.push(corrupt(format!(
                    "the body stored for `{}` is malformed: {}",
                    unit.hashes[class], bad.0
                ))),
            }
        }

        // A module never imports itself, whatever a reference inside it asked for: a unit whose
        // referent turned out to be a sibling in the same module contributed nothing to resolve.
        imports.remove(&unit.module);
        let module = modules
            .entry(unit.module.clone())
            .or_insert_with(|| Module {
                name: unit.module.clone(),
                source: Span::DUMMY.source,
                imports: Vec::new(),
                items: Vec::new(),
            });
        module.items.extend(items);
        for decl in import_decls(&imports) {
            if !module
                .imports
                .iter()
                .any(|had| had.module_name() == decl.module_name())
            {
                module.imports.push(decl);
            }
        }
    }
    let mut modules: Vec<Module> = modules.into_values().collect();

    let mut test_keys = Vec::with_capacity(bodies.tests.len());
    if !bodies.tests.is_empty() {
        let module = ModuleName::from_dotted(TEST_MODULE);
        let mut items = Vec::with_capacity(bodies.tests.len());
        let mut imports: BTreeSet<ModuleName> = BTreeSet::new();
        for (i, body) in bodies.tests.iter().enumerate() {
            // Per test, not per module.
            let mut slots: BTreeMap<DefHash, u32> = BTreeMap::new();
            let Some(Shape::Solo(bytes)) = body.shape() else {
                diags.push(corrupt(format!(
                    "the body stored for test {i} is malformed"
                )));
                continue;
            };
            let label = format!("t{i}");
            let mut decoder = Decoder {
                c: Cursor::new(bytes),
                layout: &layout,
                // A test belongs to no unit, so an intra-component reference cannot occur in one;
                // `unit` is only ever consulted through `REF_INDEX`, which `Decoder::node_ref`
                // rejects here.
                unit: usize::MAX,
                values: 0,
                ty_params: 0,
                row_params: 0,
                imports: &mut imports,
                slots: &mut slots,
                missing: &mut missing,
                relink,
            };
            match decoder.test_def(label.clone()) {
                Ok(def) => {
                    test_keys.push(module.qualify(&Symbol::new(&label)));
                    items.push(Item::Test(Box::new(def)));
                }
                Err(bad) => diags.push(corrupt(format!(
                    "the body stored for test {i} is malformed: {}",
                    bad.0
                ))),
            }
        }
        modules.push(Module {
            name: module,
            source: Span::DUMMY.source,
            imports: import_decls(&imports),
            items,
        });
    }

    if !missing.is_empty() {
        diags.push(incomplete(&missing));
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    let rebuilt = Reconstruction {
        program: Program { modules },
        names,
        kinds,
        test_keys,
    };
    if relink.is_empty() {
        rebuilt.verify(bodies)?;
    }
    Ok(rebuilt)
}

impl Reconstruction {
    /// Re-hashes what was rebuilt and requires every definition to come out as the key its body was
    /// filed under.
    fn verify(&self, bodies: &BodySet) -> Result<(), Vec<Diagnostic>> {
        // `resolve` also fills defaults and named arguments, which needs the program mutably.
        let mut expanded = self.program.clone();
        let resolved = ply_syntax::resolve(&mut expanded).map_err(|diags| {
            vec![
                corrupt("the reconstructed program does not resolve").note(format!(
                    "first: {}",
                    diags.first().map_or_else(String::new, |d| d.to_string())
                )),
            ]
        })?;
        let again = crate::hash_program_ast(&self.program, &resolved).map_err(|diags| {
            vec![
                corrupt("the reconstructed program does not hash").note(format!(
                    "first: {}",
                    diags.first().map_or_else(String::new, |d| d.to_string())
                )),
            ]
        })?;

        let mut wrong: Vec<DefHash> = Vec::new();
        for (hash, name) in &self.names {
            let rebuilt = again.defs.get(name).or_else(|| again.decls.get(name));
            if rebuilt != Some(hash) {
                wrong.push(*hash);
            }
        }
        for (i, body) in bodies.tests.iter().enumerate() {
            if again.tests.get(i).copied() != body.key() {
                wrong.push(body.key().unwrap_or(DefHash([0; 32])));
            }
        }
        if wrong.is_empty() {
            return Ok(());
        }
        let named: Vec<String> = wrong.iter().take(8).map(|h| h.short()).collect();
        Err(vec![
            corrupt(format!(
                "{} stored {} rebuild into a different definition",
                wrong.len(),
                if wrong.len() == 1 { "body" } else { "bodies" }
            ))
            .note(format!("affected: {}", named.join(", ")))
            .note(
                "mutually recursive definitions are the known case: a component's bytes label \
                 intra-component references under a coarser partition than the one that names its \
                 members, so which member calls which is not recoverable from them",
            ),
        ])
    }
}

fn import_decls(modules: &BTreeSet<ModuleName>) -> Vec<ImportDecl> {
    modules
        .iter()
        .map(|module| ImportDecl {
            path: module.segments().map(ident).collect(),
            kind: ImportKind::Module,
            span: Span::DUMMY,
        })
        .collect()
}

struct Bad(String);

fn bad(message: impl Into<String>) -> Bad {
    Bad(message.into())
}

type Decoded<T> = Result<T, Bad>;

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Cursor<'a> {
        Cursor { bytes, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos == self.bytes.len()
    }

    fn bytes(&mut self, n: usize) -> Decoded<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| bad("the stream ends inside a value"))?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u8(&mut self) -> Decoded<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u32(&mut self) -> Decoded<u32> {
        let raw: [u8; 4] = self.bytes(4)?.try_into().expect("four bytes");
        Ok(u32::from_le_bytes(raw))
    }

    fn i64(&mut self) -> Decoded<i64> {
        let raw: [u8; 8] = self.bytes(8)?.try_into().expect("eight bytes");
        Ok(i64::from_le_bytes(raw))
    }

    fn i128(&mut self) -> Decoded<i128> {
        let raw: [u8; 16] = self.bytes(16)?.try_into().expect("sixteen bytes");
        Ok(i128::from_le_bytes(raw))
    }

    /// The bit pattern, so a NaN payload and the sign of a zero survive the round trip.
    fn float(&mut self) -> Decoded<f64> {
        let raw: [u8; 8] = self.bytes(8)?.try_into().expect("eight bytes");
        Ok(f64::from_bits(u64::from_le_bytes(raw)))
    }

    fn boolean(&mut self) -> Decoded<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(bad(format!("`{other}` is not a boolean"))),
        }
    }

    fn text(&mut self) -> Decoded<Symbol> {
        let len = self.u32()? as usize;
        let raw = self.bytes(len)?;
        std::str::from_utf8(raw)
            .map(Symbol::new)
            .map_err(|_| bad("a name is not valid UTF-8"))
    }

    fn expect(&mut self, want: u8, what: &str) -> Decoded<()> {
        let got = self.u8()?;
        if got == want {
            Ok(())
        } else {
            Err(bad(format!("expected {what}, found tag {got}")))
        }
    }
}

/// The parser bounds nesting, but a left-leaning operator chain is parsed iteratively and is still
/// an arbitrarily deep tree, so this walk is as unbounded as the normalizer's.
fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

struct Decoder<'a> {
    c: Cursor<'a>,
    layout: &'a Layout,
    unit: usize,
    values: u32,
    ty_params: u32,
    row_params: u32,
    imports: &'a mut BTreeSet<ModuleName>,
    /// Effect hash -> the slot it was seen at.
    slots: &'a mut BTreeMap<DefHash, u32>,
    missing: &'a mut BTreeSet<DefHash>,
    /// Where a stored reference is redirected before it is resolved.
    relink: &'a BTreeMap<DefHash, DefHash>,
}

impl Decoder<'_> {
    fn item(&mut self, name: Symbol) -> Decoded<(Item, ItemKind)> {
        match self.c.bytes.first() {
            Some(&tag::FN) => Ok((Item::Fn(Box::new(self.fn_def(name)?)), ItemKind::Fn)),
            Some(&tag::TYPE) => Ok((Item::Type(Box::new(self.type_def(name)?)), ItemKind::Type)),
            Some(&tag::EFFECT) => Ok((
                Item::Effect(Box::new(self.effect_def(name)?)),
                ItemKind::Effect,
            )),
            Some(other) => Err(bad(format!("tag {other} does not begin a definition"))),
            None => Err(bad("the body is empty")),
        }
    }

    /// Everything is `pub`: a reconstructed program is one namespace of synthesized names, and
    /// visibility is metadata the encoding erased.
    fn fn_def(&mut self, name: Symbol) -> Decoded<FnDef> {
        self.c.expect(tag::FN, "a function")?;
        let type_count = self.c.u32()?;
        let effect_count = self.c.u32()?;
        let generics = Generics {
            types: (0..type_count)
                .map(|i| ident(ty_param_name(self.ty_params + i)))
                .collect(),
            effects: (0..effect_count)
                .map(|i| ident(row_param_name(self.row_params + i)))
                .collect(),
        };
        self.ty_params += type_count;
        self.row_params += effect_count;

        let count = self.c.u32()?;
        let annotations = self.repeat(count, Self::param_slot)?;
        let ret = self.opt(Self::type_expr)?;
        let effects = self.opt(Self::row)?;
        let constraints = self.constraints()?;

        let params = annotations
            .into_iter()
            .map(|(ty, default)| {
                let param = Param {
                    name: ident(local_name(self.values)),
                    ty,
                    default,
                    span: Span::DUMMY,
                };
                self.values += 1;
                param
            })
            .collect();
        let body = self.expr()?;
        self.end()?;
        Ok(FnDef {
            vis: Visibility::Public,
            name: ident(name),
            generics,
            params,
            ret,
            effects,
            constraints,
            // Provenance, erased by normalization: a decoded definition cannot say whether a human
            // or a `derive` wrote the form it decodes.
            derived: None,
            // A spec is erased by normalization, so a body decoded from its hash carries none.
            spec: Vec::new(),
            reuse: None,
            body,
            span: Span::DUMMY,
        })
    }

    /// Comes back sorted by `(parameter level, deriver)`, which is how the normalizer wrote it and
    /// is one of the semantics-preserving rewrites a decoded definition is only equal to its
    /// original up to.
    fn constraints(&mut self) -> Decoded<Vec<Constraint>> {
        let count = self.c.u32()?;
        self.repeat(count, |d| {
            d.c.expect(tag::CONSTRAINT, "a constraint")?;
            let level = d.c.u32()?;
            let tag = d.c.u8()?;
            let deriver =
                Deriver::from_tag(tag).ok_or_else(|| bad(format!("`{tag}` is not a deriver")))?;
            Ok(Constraint {
                deriver,
                deriver_span: Span::DUMMY,
                param: ident(ty_param_name(level)),
                span: Span::DUMMY,
            })
        })
    }

    fn type_def(&mut self, name: Symbol) -> Decoded<TypeDef> {
        self.c.expect(tag::TYPE, "a type")?;
        let count = self.c.u32()?;
        let params: Vec<Ident> = (0..count)
            .map(|i| ident(ty_param_name(self.ty_params + i)))
            .collect();
        self.ty_params += count;

        let body = match self.c.u8()? {
            tag::TYPE_ALIAS => TypeDefBody::Alias(self.type_expr()?),
            tag::TYPE_SUM => {
                let count = self.c.u32()?;
                let mut variants = Vec::with_capacity(self.hint(count));
                for _ in 0..count {
                    self.c.expect(tag::VARIANT, "a variant")?;
                    let name = self.c.text()?;
                    let fields = self.c.u32()?;
                    variants.push(VariantDef {
                        name: ident(name),
                        fields: self.repeat(fields, Self::type_expr)?,
                        span: Span::DUMMY,
                    });
                }
                TypeDefBody::Sum(variants)
            }
            other => return Err(bad(format!("tag {other} does not begin a type body"))),
        };
        self.end()?;
        Ok(TypeDef {
            vis: Visibility::Public,
            name: ident(name),
            params,
            body,
            span: Span::DUMMY,
        })
    }

    fn effect_def(&mut self, name: Symbol) -> Decoded<EffectDef> {
        self.c.expect(tag::EFFECT, "an effect")?;
        let nondet = self.c.boolean()?;
        let count = self.c.u32()?;
        let mut ops = Vec::with_capacity(self.hint(count));
        for _ in 0..count {
            self.c.expect(tag::OP, "an operation")?;
            let name = self.c.text()?;
            let mode = mode_of(self.c.u8()?)?;
            let resource_param = self.c.boolean()?;
            let params = self.c.u32()?;
            ops.push(OpDef {
                name: ident(name),
                mode,
                resource_param,
                params: self.repeat(params, Self::type_expr)?,
                ret: self.type_expr()?,
                span: Span::DUMMY,
            });
        }
        self.end()?;
        Ok(EffectDef {
            vis: Visibility::Public,
            name: ident(name),
            nondet,
            ops,
            span: Span::DUMMY,
        })
    }

    fn test_def(&mut self, label: String) -> Decoded<TestDef> {
        self.c.expect(tag::TEST, "a test")?;
        let nondet = self.c.boolean()?;
        let body = self.expr()?;
        self.end()?;
        Ok(TestDef {
            name: label,
            name_span: Span::DUMMY,
            nondet,
            body,
            span: Span::DUMMY,
        })
    }

    /// Trailing bytes mean the stream was written by something that does not agree with this
    /// decoder about a shape, which is exactly the failure the encoding version exists to catch.
    fn end(&mut self) -> Decoded<()> {
        if self.c.done() {
            Ok(())
        } else {
            Err(bad(format!(
                "{} bytes remain after the definition",
                self.c.bytes.len() - self.c.pos
            )))
        }
    }

    /// `count` comes off the stream, so it may be anything.
    fn repeat<T>(&mut self, count: u32, f: impl Fn(&mut Self) -> Decoded<T>) -> Decoded<Vec<T>> {
        let mut out = Vec::with_capacity(self.hint(count));
        for _ in 0..count {
            out.push(f(self)?);
        }
        Ok(out)
    }

    fn hint(&self, count: u32) -> usize {
        (count as usize).min(self.c.bytes.len() - self.c.pos)
    }

    fn opt<T>(&mut self, f: impl FnOnce(&mut Self) -> Decoded<T>) -> Decoded<Option<T>> {
        match self.c.u8()? {
            tag::NONE => Ok(None),
            tag::SOME => f(self).map(Some),
            other => Err(bad(format!("tag {other} is not an optional marker"))),
        }
    }

    /// One `fn` parameter's annotation and default.
    fn param_slot(&mut self) -> Decoded<(Option<TypeExpr>, Option<Expr>)> {
        match self.c.u8()? {
            tag::NONE => Ok((None, None)),
            tag::SOME => Ok((Some(self.type_expr()?), None)),
            tag::PARAM_DEFAULT => {
                let ty = self.opt(Self::type_expr)?;
                Ok((ty, Some(self.expr()?)))
            }
            other => Err(bad(format!("tag {other} does not open a parameter"))),
        }
    }

    fn node_ref(&mut self) -> Decoded<DefHash> {
        match self.c.u8()? {
            tag::REF_HASH => {
                let raw: [u8; 32] = self.c.bytes(32)?.try_into().expect("thirty-two bytes");
                let stored = DefHash(raw);
                let hash = self.relink.get(&stored).copied().unwrap_or(stored);
                if !self.layout.by_hash.contains_key(&hash) {
                    self.missing.insert(hash);
                }
                Ok(hash)
            }
            tag::REF_INDEX => {
                let class = self.c.u32()? as usize;
                self.layout
                    .units
                    .get(self.unit)
                    .and_then(|u| u.hashes.get(class))
                    .copied()
                    .ok_or_else(|| bad(format!("no member {class} in this component")))
            }
            tag::REF_SELF => Err(bad(
                "an unresolved self-reference, which a stored body never carries",
            )),
            other => Err(bad(format!("tag {other} is not a reference"))),
        }
    }

    /// The name the reconstructed program gives a referenced definition, and the import that makes
    /// it reachable from the module being built.
    fn qname_of(&mut self, hash: DefHash, name: Option<Symbol>) -> QName {
        let Some(&(unit, class)) = self.layout.by_hash.get(&hash) else {
            return QName::bare(ident(name.unwrap_or_else(|| short_name('d', hash))));
        };
        let target = name.unwrap_or_else(|| self.layout.units[unit].names[class].clone());
        // Modules rather than units: with a namespace restored, two units may land in one module,
        // and a module that imported itself to reach its own definition would not resolve.
        let module = &self.layout.units[unit].module;
        if self
            .layout
            .units
            .get(self.unit)
            .is_some_and(|own| &own.module == module)
        {
            return QName::bare(ident(target));
        }
        self.imports.insert(module.clone());
        QName::qualified(ident(self.layout.units[unit].binder.clone()), ident(target))
    }

    fn value_ref(&mut self) -> Decoded<QName> {
        match self.c.bytes.get(self.c.pos) {
            Some(&tag::LOCAL) => {
                self.c.pos += 1;
                let level = self.c.u32()?;
                if level >= self.values {
                    return Err(bad(format!("local {level} is not in scope")));
                }
                Ok(QName::bare(ident(local_name(level))))
            }
            Some(&tag::CTOR) => self.ctor_ref(),
            Some(&tag::FREE) | Some(&tag::FREE_QUALIFIED) => self.free_ref(),
            _ => {
                let hash = self.node_ref()?;
                Ok(self.qname_of(hash, None))
            }
        }
    }

    fn ctor_ref(&mut self) -> Decoded<QName> {
        match self.c.bytes.get(self.c.pos) {
            Some(&tag::CTOR) => {
                self.c.pos += 1;
                let owner = self.node_ref()?;
                let name = self.c.text()?;
                Ok(self.qname_of(owner, Some(name)))
            }
            _ => self.free_ref(),
        }
    }

    fn free_ref(&mut self) -> Decoded<QName> {
        match self.c.u8()? {
            tag::FREE => Ok(QName::bare(ident(self.c.text()?))),
            tag::FREE_QUALIFIED => {
                let module = self.c.text()?;
                let name = self.c.text()?;
                Ok(QName::qualified(ident(module), ident(name)))
            }
            other => Err(bad(format!("tag {other} is not a free reference"))),
        }
    }

    /// An effect reference carries its slot in the enclosing component's effect enumeration
    /// alongside the declaration's hash, because two effects may declare byte-identical operations
    /// and still be different capabilities.
    fn effect_ref(&mut self) -> Decoded<QName> {
        match self.c.bytes.get(self.c.pos) {
            Some(&tag::FREE) | Some(&tag::FREE_QUALIFIED) => self.free_ref(),
            _ => {
                let hash = self.node_ref()?;
                let slot = self.c.u32()?;
                match self.slots.insert(hash, slot) {
                    Some(previous) if previous != slot => {
                        return Err(bad(format!(
                            "two distinct effects share the declaration `{hash}`, which a \
                             hash-keyed body cannot tell apart"
                        )));
                    }
                    _ => {}
                }
                Ok(self.qname_of(hash, None))
            }
        }
    }

    fn type_expr(&mut self) -> Decoded<TypeExpr> {
        grow(|| self.type_expr_inner())
    }

    fn type_expr_inner(&mut self) -> Decoded<TypeExpr> {
        match self.c.u8()? {
            tag::TY_CON => {
                let param = match self.c.bytes.get(self.c.pos) {
                    Some(&tag::TY_PARAM) => {
                        self.c.pos += 1;
                        let level = self.c.u32()?;
                        if level >= self.ty_params {
                            return Err(bad(format!("type parameter {level} is not in scope")));
                        }
                        Some(ident(ty_param_name(level)))
                    }
                    _ => None,
                };
                let name = match &param {
                    Some(_) => None,
                    None => Some(match self.c.bytes.get(self.c.pos) {
                        Some(&tag::FREE) | Some(&tag::FREE_QUALIFIED) => self.free_ref()?,
                        _ => {
                            let hash = self.node_ref()?;
                            self.qname_of(hash, None)
                        }
                    }),
                };
                let count = self.c.u32()?;
                let args = self.repeat(count, Self::type_expr)?;
                match (param, name) {
                    (Some(param), _) if args.is_empty() => Ok(TypeExpr::Var(param)),
                    (Some(_), _) => Err(bad("a type parameter cannot take arguments")),
                    (None, Some(name)) => Ok(TypeExpr::Con {
                        name,
                        args,
                        span: Span::DUMMY,
                    }),
                    (None, None) => Err(bad("a type constructor with no name")),
                }
            }
            tag::TY_FN => {
                let count = self.c.u32()?;
                let params = self.repeat(count, Self::type_expr)?;
                let ret = Box::new(self.type_expr()?);
                Ok(TypeExpr::Fn {
                    params,
                    ret,
                    effects: self.opt(Self::row)?,
                    span: Span::DUMMY,
                })
            }
            tag::TY_RECORD => {
                let count = self.c.u32()?;
                let mut fields = Vec::with_capacity(self.hint(count));
                for _ in 0..count {
                    let name = self.c.text()?;
                    fields.push((ident(name), self.type_expr()?));
                }
                Ok(TypeExpr::Record {
                    fields,
                    span: Span::DUMMY,
                })
            }
            tag::TY_UNIT => Ok(TypeExpr::Unit { span: Span::DUMMY }),
            other => Err(bad(format!("tag {other} does not begin a type"))),
        }
    }

    fn row(&mut self) -> Decoded<RowExpr> {
        self.c.expect(tag::ROW, "an effect row")?;
        let count = self.c.u32()?;
        let mut atoms = Vec::with_capacity(self.hint(count));
        for _ in 0..count {
            self.c.expect(tag::ATOM, "an effect atom")?;
            let effect = self.effect_ref()?;
            let mode = mode_of(self.c.u8()?)?;
            let resource = self.opt(|s| Ok(ident(s.c.text()?)))?;
            atoms.push(AtomExpr {
                effect,
                mode,
                resource,
                span: Span::DUMMY,
            });
        }
        let tail = self.opt(|s| match s.c.u8()? {
            tag::ROW_PARAM => {
                let level = s.c.u32()?;
                if level >= s.row_params {
                    return Err(bad(format!("row parameter {level} is not in scope")));
                }
                Ok(ident(row_param_name(level)))
            }
            tag::FREE => Ok(ident(s.c.text()?)),
            other => Err(bad(format!("tag {other} is not a row tail"))),
        })?;
        Ok(RowExpr {
            atoms,
            // A decoded body is the normalized form, where an alias name was erased.
            aliases: Vec::new(),
            tail,
            span: Span::DUMMY,
        })
    }

    fn expr(&mut self) -> Decoded<Expr> {
        grow(|| self.expr_inner())
    }

    fn expr_inner(&mut self) -> Decoded<Expr> {
        let kind = match self.c.u8()? {
            tag::E_LIT => ExprKind::Lit(self.lit()?),
            tag::E_VAR => ExprKind::Var(self.value_ref()?),
            tag::E_BINARY => {
                let op = binop_of(self.c.u8()?)?;
                ExprKind::Binary {
                    op,
                    lhs: Box::new(self.expr()?),
                    rhs: Box::new(self.expr()?),
                }
            }
            tag::E_UNARY => {
                let op = unop_of(self.c.u8()?)?;
                ExprKind::Unary {
                    op,
                    operand: Box::new(self.expr()?),
                }
            }
            tag::E_LAMBDA => {
                let count = self.c.u32()?;
                let annotations = self.repeat(count, |d| d.opt(Self::type_expr))?;
                let mark = self.values;
                let params = annotations
                    .into_iter()
                    .map(|ty| {
                        let param = Param {
                            name: ident(local_name(self.values)),
                            ty,
                            // A lambda parameter cannot carry one, so the encoding has none to
                            // hold.
                            default: None,
                            span: Span::DUMMY,
                        };
                        self.values += 1;
                        param
                    })
                    .collect();
                let body = Box::new(self.expr()?);
                self.values = mark;
                ExprKind::Lambda {
                    params,
                    body,
                    ret: None,
                }
            }
            tag::E_APP => {
                let func = Box::new(self.expr()?);
                let count = self.c.u32()?;
                ExprKind::App {
                    func,
                    args: self.repeat(count, Self::expr)?,
                    // The encoding never held a named argument: `resolve` placed every one before
                    // anything hashed.
                    named: Vec::new(),
                }
            }
            tag::E_IF => ExprKind::If {
                cond: Box::new(self.expr()?),
                then_branch: Box::new(self.expr()?),
                else_branch: Box::new(self.expr()?),
            },
            tag::E_MATCH => {
                let scrutinee = Box::new(self.expr()?);
                let count = self.c.u32()?;
                let mut arms = Vec::with_capacity(self.hint(count));
                for _ in 0..count {
                    self.c.expect(tag::ARM, "a match arm")?;
                    let mark = self.values;
                    let pat = self.pattern()?;
                    let guard = self.opt(Self::expr)?;
                    let body = self.expr()?;
                    self.values = mark;
                    arms.push(MatchArm {
                        pat,
                        guard,
                        body,
                        span: Span::DUMMY,
                    });
                }
                ExprKind::Match { scrutinee, arms }
            }
            tag::E_BLOCK => {
                let mark = self.values;
                let count = self.c.u32()?;
                let stmts = self.repeat(count, Self::stmt)?;
                let tail = self.opt(Self::expr)?.map(Box::new);
                self.values = mark;
                ExprKind::Block { stmts, tail }
            }
            tag::E_RECORD => {
                let count = self.c.u32()?;
                let mut fields = Vec::with_capacity(self.hint(count));
                for _ in 0..count {
                    let name = self.c.text()?;
                    fields.push((ident(name), self.expr()?));
                }
                ExprKind::Record { fields }
            }
            tag::E_FIELD => {
                let base = Box::new(self.expr()?);
                ExprKind::Field {
                    base,
                    field: ident(self.c.text()?),
                }
            }
            tag::E_LIST => {
                let count = self.c.u32()?;
                ExprKind::List {
                    items: self.repeat(count, Self::expr)?,
                }
            }
            tag::E_PERFORM => {
                let effect = self.effect_ref()?;
                let op = ident(self.c.text()?);
                let resource = self.opt(|s| Ok(ident(s.c.text()?)))?;
                let count = self.c.u32()?;
                ExprKind::Perform {
                    effect,
                    op,
                    resource,
                    args: self.repeat(count, Self::expr)?,
                }
            }
            tag::E_HANDLE => {
                let body = Box::new(self.expr()?);
                let count = self.c.u32()?;
                let mut clauses = Vec::with_capacity(self.hint(count));
                for _ in 0..count {
                    self.c.expect(tag::CLAUSE, "a handler clause")?;
                    let effect = self.effect_ref()?;
                    let op = ident(self.c.text()?);
                    let resource = self.opt(|s| Ok(ident(s.c.text()?)))?;
                    let params = self.c.u32()?;
                    let mark = self.values;
                    let params = (0..params)
                        .map(|_| {
                            let name = ident(local_name(self.values));
                            self.values += 1;
                            name
                        })
                        .collect();
                    let resume = self.opt(|s| {
                        let name = ident(local_name(s.values));
                        s.values += 1;
                        Ok(name)
                    })?;
                    let body = self.expr()?;
                    self.values = mark;
                    clauses.push(HandleClause {
                        effect,
                        op,
                        resource,
                        params,
                        resume,
                        body,
                        span: Span::DUMMY,
                    });
                }
                let return_clause = self
                    .opt(|s| {
                        s.c.expect(tag::RETURN_CLAUSE, "a return clause")?;
                        let mark = s.values;
                        let binder = ident(local_name(s.values));
                        s.values += 1;
                        let body = s.expr()?;
                        s.values = mark;
                        Ok(ReturnClause {
                            binder,
                            body,
                            span: Span::DUMMY,
                        })
                    })?
                    .map(Box::new);
                ExprKind::Handle {
                    body,
                    clauses,
                    return_clause,
                }
            }
            tag::E_WITH_CELL => {
                let resource = ident(self.c.text()?);
                let init = Box::new(self.expr()?);
                let mark = self.values;
                let binder = ident(local_name(self.values));
                self.values += 1;
                let body = Box::new(self.expr()?);
                self.values = mark;
                ExprKind::WithCell {
                    resource,
                    init,
                    binder,
                    body,
                }
            }
            tag::E_WITH_REGION => ExprKind::WithRegion {
                region: ident(self.c.text()?),
                body: Box::new(self.expr()?),
            },
            tag::E_SIMULATE => ExprKind::Simulate {
                body: Box::new(self.expr()?),
            },
            other => return Err(bad(format!("tag {other} does not begin an expression"))),
        };
        Ok(Expr {
            kind,
            span: Span::DUMMY,
        })
    }

    fn stmt(&mut self) -> Decoded<Stmt> {
        match self.c.u8()? {
            tag::S_LET => {
                let ty = self.opt(Self::type_expr)?;
                let value = Box::new(self.expr()?);
                let pat = self.pattern()?;
                Ok(Stmt::Let {
                    pat,
                    ty,
                    value,
                    span: Span::DUMMY,
                })
            }
            tag::S_EXPR => Ok(Stmt::Expr(self.expr()?)),
            other => Err(bad(format!("tag {other} does not begin a statement"))),
        }
    }

    fn lit(&mut self) -> Decoded<Lit> {
        match self.c.u8()? {
            tag::LIT_INT => Ok(Lit::Int(self.c.i64()?)),
            tag::LIT_BOOL => Ok(Lit::Bool(self.c.boolean()?)),
            tag::LIT_STR => Ok(Lit::Str(self.c.text()?.to_string())),
            tag::LIT_BYTES => {
                let len = self.c.u32()? as usize;
                Ok(Lit::Bytes(self.c.bytes(len)?.to_vec()))
            }
            tag::LIT_FLOAT => Ok(Lit::Float(self.c.float()?)),
            tag::LIT_DECIMAL => {
                let mantissa = self.c.i128()?;
                let scale = self.c.u32()?;
                // The lexer refuses these bounds, so no body this repository wrote can carry one —
                // which is exactly why a stream that does is refused rather than turned into a
                // value the evaluator would have to invent.
                if scale > MAX_DECIMAL_SCALE || mantissa.unsigned_abs() > MAX_DECIMAL_MANTISSA {
                    return Err(bad(format!(
                        "mantissa {mantissa} at scale {scale} is not a `Decimal`"
                    )));
                }
                Ok(Lit::Decimal { mantissa, scale })
            }
            tag::LIT_UNIT => Ok(Lit::Unit),
            other => Err(bad(format!("tag {other} is not a literal"))),
        }
    }

    fn pattern(&mut self) -> Decoded<Pattern> {
        grow(|| self.pattern_inner())
    }

    fn pattern_inner(&mut self) -> Decoded<Pattern> {
        let kind = match self.c.u8()? {
            tag::P_WILDCARD => PatternKind::Wildcard,
            tag::P_VAR => {
                let name = ident(local_name(self.values));
                self.values += 1;
                PatternKind::Var(name)
            }
            tag::P_LIT => PatternKind::Lit(self.lit()?),
            tag::P_CTOR => {
                let name = self.ctor_ref()?;
                let count = self.c.u32()?;
                PatternKind::Ctor {
                    name,
                    args: self.repeat(count, Self::pattern)?,
                }
            }
            tag::P_RECORD => {
                let count = self.c.u32()?;
                let mut fields = Vec::with_capacity(self.hint(count));
                for _ in 0..count {
                    let name = self.c.text()?;
                    fields.push((ident(name), self.pattern()?));
                }
                PatternKind::Record {
                    fields,
                    rest: self.c.boolean()?,
                }
            }
            tag::P_LIST => {
                let count = self.c.u32()?;
                let items = self.repeat(count, Self::pattern)?;
                PatternKind::List {
                    items,
                    rest: self.opt(Self::pattern)?.map(Box::new),
                }
            }
            other => return Err(bad(format!("tag {other} does not begin a pattern"))),
        };
        Ok(Pattern {
            kind,
            span: Span::DUMMY,
        })
    }
}

/// The normalizer's byte table is the source of truth; this is its inverse, pinned by a round-trip
/// test over every operator.
fn binop_of(byte: u8) -> Decoded<BinOp> {
    const ALL: [BinOp; 20] = [
        BinOp::Add,
        BinOp::Sub,
        BinOp::Mul,
        BinOp::Div,
        BinOp::Rem,
        BinOp::Eq,
        BinOp::Ne,
        BinOp::Lt,
        BinOp::Le,
        BinOp::Gt,
        BinOp::Ge,
        BinOp::And,
        BinOp::Or,
        BinOp::Concat,
        BinOp::BitAnd,
        BinOp::BitOr,
        BinOp::BitXor,
        BinOp::Shl,
        BinOp::Shr,
        BinOp::Ushr,
    ];
    ALL.into_iter()
        .find(|op| binop_byte(*op) == byte)
        .ok_or_else(|| bad(format!("`{byte}` is not a binary operator")))
}

fn unop_of(byte: u8) -> Decoded<UnOp> {
    [UnOp::Neg, UnOp::Not, UnOp::BitNot]
        .into_iter()
        .find(|op| unop_byte(*op) == byte)
        .ok_or_else(|| bad(format!("`{byte}` is not a unary operator")))
}

fn mode_of(byte: u8) -> Decoded<Mode> {
    [Mode::Read, Mode::Write]
        .into_iter()
        .find(|m| mode_byte(*m) == byte)
        .ok_or_else(|| bad(format!("`{byte}` is not an access mode")))
}
