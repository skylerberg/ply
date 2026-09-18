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

/// The bodies out of a front end's answer, keyed the way the store files them.
///
/// The inverse of `ply_codegen::source`'s `fill_bodies`, which writes each body as
/// [`StoredBody::as_bytes`]; `from_bytes` reads that envelope back, so `key()` re-derives the
/// hash the definition is filed under rather than being told it. A name declared in two
/// namespaces has two bodies and one entry per hash, which is the case `verify` settles.
pub fn of_front(front: &ply_ty::Front) -> BodySet {
    let hashes = &front.hashes;
    let mut by_name: std::collections::BTreeMap<&Symbol, Vec<StoredBody>> = Default::default();
    for (name, bytes) in &front.bodies {
        if let Some(body) = StoredBody::from_bytes(bytes.clone()) {
            by_name.entry(name).or_default().push(body);
        }
    }
    let mut out = BodySet::default();
    for (name, stored) in by_name {
        for hash in [hashes.defs.get(name), hashes.decls.get(name)]
            .into_iter()
            .flatten()
        {
            let found = match stored.as_slice() {
                [only] => Some(only),
                many => many.iter().find(|b| b.verify(*hash)),
            };
            if let Some(body) = found {
                out.insert(*hash, body.clone());
            }
        }
    }
    // Parallel to `CheckOutput::tests`, which the protocol checks when it decodes the frames.
    for bytes in &front.test_bodies {
        if let Some(body) = StoredBody::from_bytes(bytes.clone()) {
            out.push_test(body);
        }
    }
    out
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
    /// Whether the unit is decoded, rather than only named so a reference into it can be written.
    rebuilt: bool,
}

struct Layout {
    units: Vec<Unit>,
    /// Hash -> every (unit, class) declaring it.
    by_hash: BTreeMap<DefHash, Vec<(usize, usize)>>,
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

/// The component a body belongs to: its id, its members' encodings in class order, and whether it
/// is a solo definition rather than a cycle.
fn component_of(hash: DefHash, body: &StoredBody) -> Result<(DefHash, Vec<Vec<u8>>, bool), String> {
    let (id, members, solo) = match body.shape() {
        None => {
            return Err(format!(
                "the body stored for `{hash}` is not a definition body"
            ));
        }
        Some(Shape::Solo(bytes)) => (DefHash::of(bytes), vec![bytes.to_vec()], true),
        Some(Shape::Member { class, payload }) => {
            let Some(members) = unpack(payload) else {
                return Err(format!(
                    "the component body stored for `{hash}` is malformed"
                ));
            };
            if class as usize >= members.len() {
                return Err(format!(
                    "the body stored for `{hash}` names member {class} of a component with {} of \
                     them",
                    members.len()
                ));
            }
            (DefHash::of(payload), members, false)
        }
    };
    if !body.verify(hash) {
        return Err(format!(
            "the body stored for `{hash}` hashes to `{}`",
            body.key().map_or_else(|| "nothing".into(), |h| h.short())
        ));
    }
    Ok((id, members, solo))
}

fn member_hashes(id: DefHash, width: usize, solo: bool) -> Vec<DefHash> {
    if solo {
        vec![id]
    } else {
        (0..width as u32).map(|i| member_hash(id, i)).collect()
    }
}

impl Layout {
    fn build(bodies: &BodySet) -> Result<Layout, Vec<Diagnostic>> {
        let mut units: IndexMap<DefHash, Unit> = IndexMap::new();
        let mut diags = Vec::new();
        for (hash, body) in bodies.defs() {
            let (id, members, solo) = match component_of(hash, body) {
                Ok(component) => component,
                Err(message) => {
                    diags.push(corrupt(message));
                    continue;
                }
            };
            units.entry(id).or_insert_with(|| {
                let hashes = member_hashes(id, members.len(), solo);
                Unit {
                    id,
                    names: hashes.iter().map(|h| short_name('d', *h)).collect(),
                    hashes,
                    members,
                    module: ModuleName::from_dotted(short_name('m', id).as_str()),
                    binder: short_name('m', id),
                    rebuilt: true,
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
        Ok(Layout::index(units))
    }

    fn index(units: Vec<Unit>) -> Layout {
        let mut by_hash: BTreeMap<DefHash, Vec<(usize, usize)>> = BTreeMap::new();
        for (u, unit) in units.iter().enumerate() {
            for (class, hash) in unit.hashes.iter().enumerate() {
                by_hash.entry(*hash).or_default().push((u, class));
            }
        }
        Layout { units, by_hash }
    }

    /// The declaration a reference from `from` is written as when several carry its hash: one in
    /// that module, else one that is rebuilt, else the first.
    fn pick(&self, hash: DefHash, from: Option<&ModuleName>) -> Option<(usize, usize)> {
        self.by_hash
            .get(&hash)?
            .iter()
            .min_by_key(|(u, _)| {
                let unit = &self.units[*u];
                (Some(&unit.module) != from, !unit.rebuilt)
            })
            .copied()
    }
}

/// Every rebuilt unit decoded into the module it names.
struct Rebuilt {
    modules: IndexMap<ModuleName, Module>,
    names: IndexMap<DefHash, Symbol>,
    kinds: IndexMap<DefHash, ItemKind>,
    /// A member that did not decode, and why.
    malformed: Vec<(DefHash, String)>,
    /// Hashes a body refers to that no unit declares.
    missing: BTreeSet<DefHash>,
}

fn rebuild(layout: &Layout, relink: &BTreeMap<DefHash, DefHash>) -> Rebuilt {
    let mut out = Rebuilt {
        modules: IndexMap::new(),
        names: IndexMap::new(),
        kinds: IndexMap::new(),
        malformed: Vec::new(),
        missing: BTreeSet::new(),
    };
    for (u, unit) in layout.units.iter().enumerate() {
        if !unit.rebuilt {
            continue;
        }
        let mut items = Vec::with_capacity(unit.members.len());
        let mut imports: BTreeMap<ModuleName, Symbol> = BTreeMap::new();
        let mut slots: BTreeMap<DefHash, u32> = BTreeMap::new();

        for (class, encoding) in unit.members.iter().enumerate() {
            let mut decoder = Decoder {
                c: Cursor::new(encoding),
                layout,
                unit: u,
                values: 0,
                ty_params: 0,
                row_params: 0,
                imports: &mut imports,
                slots: &mut slots,
                missing: &mut out.missing,
                relink,
            };
            match decoder.item(unit.names[class].clone()) {
                Ok((item, kind)) => {
                    out.names
                        .insert(unit.hashes[class], unit.module.qualify(&unit.names[class]));
                    out.kinds.insert(unit.hashes[class], kind);
                    items.push(item);
                }
                Err(bad) => out.malformed.push((unit.hashes[class], bad.0)),
            }
        }

        // A module never imports itself, whatever a reference inside it asked for: a unit whose
        // referent turned out to be a sibling in the same module contributed nothing to resolve.
        imports.remove(&unit.module);
        // Keyed by name rather than pushed per unit: several units may belong to one module, and
        // two `Module`s of one name is not a program.
        let module = out
            .modules
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
    out
}

/// Rebuilds a checkable, evaluable program from stored bodies.
pub fn reconstruct(bodies: &BodySet) -> Result<Reconstruction, Vec<Diagnostic>> {
    reconstruct_relinked(bodies, &BTreeMap::new())
}

/// [`reconstruct`], with every stored reference rewritten through `relink` before it is resolved.
pub fn reconstruct_relinked(
    bodies: &BodySet,
    relink: &BTreeMap<DefHash, DefHash>,
) -> Result<Reconstruction, Vec<Diagnostic>> {
    let layout = Layout::build(bodies)?;
    let rebuilt = rebuild(&layout, relink);
    let mut diags: Vec<Diagnostic> = rebuilt
        .malformed
        .iter()
        .map(|(hash, why)| corrupt(format!("the body stored for `{hash}` is malformed: {why}")))
        .collect();
    let mut missing = rebuilt.missing;
    let mut modules: Vec<Module> = rebuilt.modules.into_values().collect();

    let mut test_keys = Vec::with_capacity(bodies.tests.len());
    if !bodies.tests.is_empty() {
        let module = ModuleName::from_dotted(TEST_MODULE);
        let mut items = Vec::with_capacity(bodies.tests.len());
        let mut imports: BTreeMap<ModuleName, Symbol> = BTreeMap::new();
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
    let out = Reconstruction {
        program: Program { modules },
        names: rebuilt.names,
        kinds: rebuilt.kinds,
        test_keys,
    };
    if relink.is_empty() {
        out.verify(bodies)?;
    }
    Ok(out)
}

/// A program rebuilt from `bodies` under exactly the program-wide names `names` gives them: one
/// item per name, in that name's module, imported `as` another binder wherever two modules share a
/// last segment. Nothing is invented and nothing is dropped, and names that cannot say which
/// declaration a body means are refused rather than guessed.
///
/// A module `named_only` is named, so that a reference into it can be written, and not rebuilt.
pub fn reconstruct_exact(
    bodies: &BodySet,
    names: &[(Symbol, DefHash)],
    named_only: impl Fn(&ModuleName) -> bool,
) -> Result<Program, Vec<Diagnostic>> {
    let mut named: BTreeMap<DefHash, Vec<(ModuleName, Symbol)>> = BTreeMap::new();
    for (name, hash) in names {
        let Some((module, simple)) = name.as_str().rsplit_once('.') else {
            return Err(vec![refused(format!("`{name}` names no module"))]);
        };
        if !bodies.contains(*hash) {
            return Err(vec![refused(format!("`{name}` has no body"))]);
        }
        named
            .entry(*hash)
            .or_default()
            .push((ModuleName::from_dotted(module), Symbol::new(simple)));
    }
    for all in named.values_mut() {
        all.sort();
        all.dedup();
    }

    let mut units: Vec<Unit> = Vec::new();
    let mut seen: BTreeSet<DefHash> = BTreeSet::new();
    let mut declared: BTreeMap<(u8, Symbol), DefHash> = BTreeMap::new();
    for (hash, body) in bodies.defs() {
        let (id, members, solo) = component_of(hash, body).map_err(|why| vec![refused(why)])?;
        if !seen.insert(id) {
            continue;
        }
        let hashes = member_hashes(id, members.len(), solo);
        let mut classes: Vec<&[(ModuleName, Symbol)]> = Vec::with_capacity(hashes.len());
        for (member, encoding) in hashes.iter().zip(&members) {
            let all: &[(ModuleName, Symbol)] =
                named.get(member).map(Vec::as_slice).unwrap_or_default();
            if all.is_empty() {
                return Err(vec![refused(format!(
                    "the body `{}` has no name",
                    member.short()
                ))]);
            }
            let kind = encoding.first().copied().unwrap_or_default();
            if kind == tag::EFFECT && all.len() > 1 {
                return Err(vec![
                    refused(format!(
                        "{} are one effect declaration, so a body cannot say which it performs",
                        listed(all)
                    ))
                    .note("make the declarations differ, or keep one of them"),
                ]);
            }
            for (module, simple) in all {
                let qualified = module.qualify(simple);
                if let Some(other) = declared.insert((kind, qualified.clone()), *member)
                    && other != *member
                {
                    return Err(vec![refused(format!(
                        "`{qualified}` names two different definitions"
                    ))]);
                }
            }
            classes.push(all);
        }
        let Some(sets) = copies(&classes) else {
            let all: Vec<(ModuleName, Symbol)> = classes.concat();
            return Err(vec![
                refused(format!(
                    "{} are a mutually recursive group in which two definitions share one body, so \
                     a call cannot say which it reaches",
                    listed(&all)
                ))
                .note("make the definitions differ, or keep one of them"),
            ]);
        };
        for (module, names) in sets {
            units.push(Unit {
                id,
                members: members.clone(),
                hashes: hashes.clone(),
                names,
                binder: module.default_binder(),
                rebuilt: !named_only(&module),
                module,
            });
        }
    }

    let bound = binders(units.iter().map(|unit| &unit.module).collect());
    for unit in &mut units {
        unit.binder = bound[&unit.module].clone();
    }
    let layout = Layout::index(units);
    let rebuilt = rebuild(&layout, &BTreeMap::new());
    if let Some((hash, why)) = rebuilt.malformed.first() {
        return Err(vec![refused(format!(
            "the body `{}` does not decode: {why}",
            hash.short()
        ))]);
    }
    if !rebuilt.missing.is_empty() {
        let absent: Vec<String> = rebuilt.missing.iter().take(8).map(|h| h.short()).collect();
        return Err(vec![
            refused(format!(
                "{} definitions a body refers to are not among the bodies",
                rebuilt.missing.len()
            ))
            .note(format!("missing: {}", absent.join(", "))),
        ]);
    }
    let mut modules: Vec<Module> = rebuilt.modules.into_values().collect();
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Program { modules })
}

fn refused(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::ARTIFACT_INVALID, message)
}

fn listed(names: &[(ModuleName, Symbol)]) -> String {
    let quoted: Vec<String> = names
        .iter()
        .map(|(module, name)| format!("`{}`", module.qualify(name)))
        .collect();
    quoted.join(", ")
}

/// A component's names as whole copies, one name per class. With one class every name is a copy
/// of its own; otherwise a module's `n`th name of each class make one. `None` when a module names
/// one class more often than another, which no set of copies can say.
fn copies(classes: &[&[(ModuleName, Symbol)]]) -> Option<Vec<(ModuleName, Vec<Symbol>)>> {
    if let [only] = classes {
        return Some(
            only.iter()
                .map(|(module, name)| (module.clone(), vec![name.clone()]))
                .collect(),
        );
    }
    let mut by_module: BTreeMap<&ModuleName, Vec<Vec<&Symbol>>> = BTreeMap::new();
    for (class, all) in classes.iter().enumerate() {
        for (module, name) in all.iter() {
            by_module
                .entry(module)
                .or_insert_with(|| vec![Vec::new(); classes.len()])[class]
                .push(name);
        }
    }
    let mut out = Vec::new();
    for (module, by_class) in by_module {
        let count = by_class[0].len();
        if by_class.iter().any(|names| names.len() != count) {
            return None;
        }
        for n in 0..count {
            out.push((
                module.clone(),
                by_class.iter().map(|names| names[n].clone()).collect(),
            ));
        }
    }
    Some(out)
}

/// Each module's last segment, unless another module shares it: then each of those is bound as its
/// whole path joined with `_`, suffixed until nothing else is bound so.
fn binders(modules: BTreeSet<&ModuleName>) -> BTreeMap<ModuleName, Symbol> {
    let mut sharing: BTreeMap<Symbol, usize> = BTreeMap::new();
    for module in &modules {
        *sharing.entry(module.default_binder()).or_default() += 1;
    }
    let mut taken: BTreeSet<Symbol> = sharing.keys().cloned().collect();
    modules
        .into_iter()
        .map(|module| {
            let own = module.default_binder();
            if sharing[&own] == 1 {
                return (module.clone(), own);
            }
            let base = module.segments().collect::<Vec<_>>().join("_");
            let mut binder = Symbol::new(&base);
            let mut n = 1;
            while !taken.insert(binder.clone()) {
                n += 1;
                binder = Symbol::new(format!("{base}_{n}"));
            }
            (module.clone(), binder)
        })
        .collect()
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

fn import_decls(modules: &BTreeMap<ModuleName, Symbol>) -> Vec<ImportDecl> {
    modules
        .iter()
        .map(|(module, binder)| ImportDecl {
            path: module.segments().map(ident).collect(),
            kind: if *binder == module.default_binder() {
                ImportKind::Module
            } else {
                ImportKind::Alias(ident(binder.clone()))
            },
            span: Span::DUMMY,
        })
        .collect()
}

/// A stored reference: a definition by hash, or a member of the component being decoded.
#[derive(Clone, Copy)]
enum Ref {
    Hash(DefHash),
    Class(usize),
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
    /// Module -> the binder a reference into it is written with.
    imports: &'a mut BTreeMap<ModuleName, Symbol>,
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

    /// Everything is `pub`: visibility is metadata the encoding erased.
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

    fn node_ref(&mut self) -> Decoded<Ref> {
        match self.c.u8()? {
            tag::REF_HASH => {
                let raw: [u8; 32] = self.c.bytes(32)?.try_into().expect("thirty-two bytes");
                let stored = DefHash(raw);
                let hash = self.relink.get(&stored).copied().unwrap_or(stored);
                if !self.layout.by_hash.contains_key(&hash) {
                    self.missing.insert(hash);
                }
                Ok(Ref::Hash(hash))
            }
            tag::REF_INDEX => {
                let class = self.c.u32()? as usize;
                match self.layout.units.get(self.unit) {
                    Some(unit) if class < unit.hashes.len() => Ok(Ref::Class(class)),
                    _ => Err(bad(format!("no member {class} in this component"))),
                }
            }
            tag::REF_SELF => Err(bad(
                "an unresolved self-reference, which a stored body never carries",
            )),
            other => Err(bad(format!("tag {other} is not a reference"))),
        }
    }

    fn hash_of(&self, target: Ref) -> DefHash {
        match target {
            Ref::Hash(hash) => hash,
            Ref::Class(class) => self.layout.units[self.unit].hashes[class],
        }
    }

    /// The name the reconstructed program gives a referenced definition, and the import that makes
    /// it reachable from the module being built. A member of this component is this copy's own.
    fn qname_of(&mut self, target: Ref, ctor: Option<Symbol>) -> QName {
        let layout = self.layout;
        let own = layout.units.get(self.unit).map(|unit| &unit.module);
        let (unit, class) = match target {
            Ref::Class(class) => (self.unit, class),
            Ref::Hash(hash) => match layout.pick(hash, own) {
                Some(at) => at,
                None => return QName::bare(ident(ctor.unwrap_or_else(|| short_name('d', hash)))),
            },
        };
        let owner = &layout.units[unit];
        let name = ident(ctor.unwrap_or_else(|| owner.names[class].clone()));
        // Modules rather than units: several units may land in one module, and a module that
        // imported itself to reach its own definition would not resolve.
        if own == Some(&owner.module) {
            return QName::bare(name);
        }
        self.imports
            .insert(owner.module.clone(), owner.binder.clone());
        QName::qualified(ident(owner.binder.clone()), name)
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
                let target = self.node_ref()?;
                Ok(self.qname_of(target, None))
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
                let target = self.node_ref()?;
                let slot = self.c.u32()?;
                let hash = self.hash_of(target);
                match self.slots.insert(hash, slot) {
                    Some(previous) if previous != slot => {
                        return Err(bad(format!(
                            "two distinct effects share the declaration `{hash}`, which a \
                             hash-keyed body cannot tell apart"
                        )));
                    }
                    _ => {}
                }
                Ok(self.qname_of(target, None))
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
                            let target = self.node_ref()?;
                            self.qname_of(target, None)
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
            tag::LIT_FIXED => {
                let which = self.c.u8()?;
                let ty = *ply_ty::INT_TYPES
                    .get(which as usize)
                    .ok_or_else(|| bad(format!("{which} is not a fixed-width integer type")))?;
                let bits = self.c.i64()? as u64;
                // Normalized on the way in as the lexer normalizes it, so a stream carrying a
                // pattern outside the width decodes to the value that width holds rather than to
                // one no program could have written.
                Ok(Lit::Fixed {
                    ty,
                    bits: ty.normalize(bits),
                })
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
