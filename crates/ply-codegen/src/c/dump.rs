//! The front end's answer, a `front.Dump` as `crates/ply-compiler/ply/front.ply` builds it, read
//! into a [`Front`]. No type is read: the runtime reasons about none, and a value's words read back
//! as the carries the compiler published. A footprint is read from the checker's own atoms, its
//! label variables numbered where they first appear, so an answer reads to one structure however
//! the checker happened to number them.

use ply_eval::decode::{At, Error};
use ply_eval::{
    Carry, DefHash, DefInfo, DefWritten, Diagnostic, Edit, EffectAtom, EffectInfo, EffectSet,
    EmitterRoot, Fix, Footprint, Front, HashOutput, Hashed, INT_TYPES, Label, LawInfo, Literal,
    Mode, ModuleInfo, ModuleName, OpInfo, Ordinal, Pinned, Resource, Severity, SourceId, Span,
    SpecInfo, SpecKind, Symbol, TestInfo, TypeDecl, Value, Visibility, WrittenParam, intern_code,
};
use std::collections::BTreeMap;

/// What every failure's path starts from.
const ANSWER: &str = "the front end's answer";

/// The module index of a span outside every module.
const NO_MODULE: u32 = u32::MAX;

/// `sources[i]` is the source a module index `i` names: the program's own modules, the shipped
/// ones pulled after them, then any manifest the answer places past those.
pub fn read(dump: &Value, sources: &[SourceId]) -> Result<Front, Error> {
    let d = At::new(ANSWER, dump);
    let r = Reader { sources };
    let mut front = Front {
        diagnostics: d.field("diags")?.items(|x| r.diagnostic(x))?,
        ..Front::default()
    };
    // An error is the whole answer: a refused program has no rows to read.
    if front.has_error() {
        return Ok(front);
    }

    front.order = symbols(d.field("order")?)?;
    front.packages = d.field("packages")?.items(|p| {
        Ok((
            p.field("prefix")?.utf8()?.to_string(),
            strings(p.field("deps")?)?,
        ))
    })?;
    front.pins = d.field("pins")?.items(|p| {
        Ok(Pinned {
            name: p.field("name")?.utf8()?.to_string(),
            version: p.field("version")?.utf8()?.to_string(),
            digest: p.field("digest")?.utf8()?.to_string(),
        })
    })?;
    front.mod_pkg = d.field("mod_pkg")?.items(|i| i.number())?;
    for m in d.field("modules")?.list()? {
        let name = m.field("name")?.utf8()?;
        let sets = m.field("sets")?.items(|s| {
            Ok(EffectSet {
                name: Symbol::new(s.field("name")?.utf8()?),
                includes: symbols(s.field("includes")?)?,
                atoms: footprint(s.field("atoms")?)?,
            })
        })?;
        if !sets.is_empty() {
            front.effect_sets.insert(Symbol::new(name), sets);
        }
        let index = m.field("index")?;
        let source = *sources.get(index.number::<usize>()?).ok_or_else(|| {
            index.error(format!(
                "a module's source, and only {} sources were handed over",
                sources.len()
            ))
        })?;
        front.check.modules.insert(
            Symbol::new(name),
            ModuleInfo {
                name: ModuleName::from_dotted(name),
                source,
                items: symbols(m.field("items")?)?,
                imports: m
                    .field("imports")?
                    .items(|i| Ok(ModuleName::from_dotted(i.utf8()?)))?,
            },
        );
    }
    for def in d.field("defs")?.list()? {
        let (info, written) = r.def(def)?;
        front.defs_written.insert(info.name.clone(), written);
        front.check.defs.insert(info.name.clone(), info);
    }
    for t in d.field("types")?.list()? {
        let decl = r.type_decl(t)?;
        front.types.insert(decl.name.clone(), decl);
    }
    for (i, t) in d.field("tests")?.list()?.enumerate() {
        let (test, name_span) = r.test(t, i)?;
        front.check.tests.push(test);
        front.test_name_spans.push(name_span);
    }
    for (i, l) in d.field("laws")?.list()?.enumerate() {
        let (law, literals) = r.law(l, i)?;
        front.check.laws.push(law);
        front.law_literals.push(literals);
    }
    for e in d.field("effects")?.list()? {
        let (effect, vis) = r.effect(e)?;
        front.effects_written.insert(effect.name.clone(), vis);
        front.check.effects.insert(effect.name.clone(), effect);
    }
    hashes(d.field("hashes")?, &mut front)?;
    front.hashes_digest = DefHash(d.field("hashes_digest")?.byte_array()?);
    for k in d.field("keys")?.list()? {
        front.keys.insert(
            Symbol::new(k.field("root")?.utf8()?),
            k.field("key")?.utf8()?.to_string(),
        );
    }
    front.emitter_roots = d.field("emit_roots")?.items(|e| {
        Ok(EmitterRoot {
            root: Symbol::new(e.field("root")?.utf8()?),
            arity: e.field("arity")?.number()?,
            scalar: e.field("scalar")?.bool()?,
            pure: e.field("pure")?.bool()?,
            span: r.span(e.field("at")?)?,
            params: published(e, "params", carries)?.unwrap_or_default(),
            answer: published(e, "answer", carry)?.unwrap_or(Carry::Open),
        })
    })?;
    for k in d.field("emit_ctors")?.list()? {
        let name = Symbol::new(k.field("name")?.utf8()?);
        if let Some(fields) = published(k, "fields", carries)? {
            front.ctor_carries.insert(name.clone(), fields);
        }
        front
            .emitter_ctors
            .push((name, k.field("arity")?.number()?));
    }
    front.ordinals = d.field("ordinals")?.items(|o| {
        Ok((
            Symbol::new(o.field("module")?.utf8()?),
            o.field("items")?.items(ordinal)?,
        ))
    })?;
    front.bodies = d.field("bodies")?.items(|b| {
        Ok((
            Symbol::new(b.field("name")?.utf8()?),
            b.field("body")?.bytes()?.to_vec(),
        ))
    })?;
    let bodies = d.field("test_bodies")?;
    front.test_bodies = bodies.items(|b| Ok(b.bytes()?.to_vec()))?;
    if front.test_bodies.len() != front.check.tests.len() {
        return Err(bodies.error(format!(
            "{} test bodies beside {} tests",
            front.test_bodies.len(),
            front.check.tests.len()
        )));
    }
    resolve_op_modes(&mut front);
    Ok(front)
}

/// The hasher's rows in its item order. A test's or a law's row is numbered by the item it is
/// about, so the tests and laws are read before them.
fn hashes(rows: At<'_>, front: &mut Front) -> Result<(), Error> {
    let tests = front.check.tests.len();
    let laws = front.check.laws.len();
    let mut test_hashes: Vec<Option<DefHash>> = vec![None; tests];
    let mut law_hashes: Vec<Option<(DefHash, DefHash)>> = vec![None; laws];
    for row in rows.list()? {
        let c = row.ctor()?;
        let h = c.arg(0)?;
        match c.name() {
            "HDef" => {
                def_hash(h, &mut front.hashes)?;
                front
                    .hash_order
                    .push(Hashed::Def(Symbol::new(h.field("name")?.utf8()?)));
            }
            "HTest" => {
                let i = item(h, tests, "test")?;
                let key = front.check.tests[i].key.clone();
                let hash = item_hash(h, &key, &mut front.hashes)?;
                if test_hashes[i].replace(hash).is_some() {
                    return Err(h.error(format!("test {i} is hashed twice")));
                }
                front.hash_order.push(Hashed::Test(i));
            }
            "HLaw" => {
                let i = item(h, laws, "law")?;
                let key = front.check.laws[i].key.clone();
                let hash = item_hash(h, &key, &mut front.hashes)?;
                let text = hash_of(h.field("text")?)?;
                if law_hashes[i].replace((hash, text)).is_some() {
                    return Err(h.error(format!("law {i} is hashed twice")));
                }
                front.hash_order.push(Hashed::Law(i));
            }
            _ => return Err(c.unknown()),
        }
    }
    for (i, hash) in test_hashes.into_iter().enumerate() {
        let hash = hash.ok_or_else(|| rows.error(format!("test {i} has no hash row")))?;
        front.hashes.tests.push(hash);
    }
    for (i, hashes) in law_hashes.into_iter().enumerate() {
        let (hash, text) = hashes.ok_or_else(|| rows.error(format!("law {i} has no hash row")))?;
        front.hashes.laws.push(hash);
        front.hashes.law_texts.push(text);
    }
    Ok(())
}

/// A `fn`'s, `type`'s or `effect`'s row: one per name, a name in two namespaces included.
fn def_hash(h: At<'_>, out: &mut HashOutput) -> Result<(), Error> {
    let name = Symbol::new(h.field("name")?.utf8()?);
    if out.deps.contains_key(&name) {
        return Err(h.error(format!("`{name}` is hashed twice")));
    }
    if let Some(x) = h.field("def")?.option()? {
        out.defs.insert(name.clone(), hash_of(x)?);
    }
    if let Some(x) = h.field("own")?.option()? {
        out.own.insert(name.clone(), hash_of(x)?);
    }
    if let Some(x) = h.field("decl")?.option()? {
        out.decls.insert(name.clone(), hash_of(x)?);
    }
    let specs = h.field("specs")?.items(hash_of)?;
    if !specs.is_empty() {
        out.specs.insert(name.clone(), specs);
    }
    let spec_texts = h.field("spec_texts")?.items(hash_of)?;
    if !spec_texts.is_empty() {
        out.spec_texts.insert(name.clone(), spec_texts);
    }
    out.deps.insert(name.clone(), symbols(h.field("deps")?)?);
    out.closure
        .insert(name, symbols(h.field("closure")?)?.into_iter().collect());
    Ok(())
}

/// The test or law a hash row numbers, which the rows before it declared.
fn item(h: At<'_>, declared: usize, of: &str) -> Result<usize, Error> {
    let index = h.field("index")?;
    let i: usize = index.number()?;
    if i >= declared {
        return Err(index.error(format!("names {of} {i}, and only {declared} were declared")));
    }
    Ok(i)
}

/// A test's or a law's hash; its references merge into what its key already holds.
fn item_hash(h: At<'_>, declared: &Symbol, out: &mut HashOutput) -> Result<DefHash, Error> {
    let key = h.field("key")?;
    if key.utf8()? != declared.as_str() {
        return Err(key.error(format!("the item this row numbers is keyed `{declared}`")));
    }
    let hash = hash_of(h.field("hash")?)?;
    let known = out.deps.entry(declared.clone()).or_default();
    for d in symbols(h.field("deps")?)? {
        if !known.contains(&d) {
            known.push(d);
        }
    }
    out.closure
        .entry(declared.clone())
        .or_default()
        .extend(symbols(h.field("closure")?)?);
    Ok(hash)
}

/// A hash as the hasher's rows hold one: sixty-four hex digits.
fn hash_of(x: At<'_>) -> Result<DefHash, Error> {
    let text = x.utf8()?;
    DefHash::from_hex(text).ok_or_else(|| x.error(format!("`{text}` is not a hash")))
}

fn symbols(list: At<'_>) -> Result<Vec<Symbol>, Error> {
    list.items(|s| Ok(Symbol::new(s.utf8()?)))
}

fn strings(list: At<'_>) -> Result<Vec<String>, Error> {
    list.items(|s| Ok(s.utf8()?.to_string()))
}

fn visibility(public: bool) -> Visibility {
    if public {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

fn spec_kind(x: At<'_>) -> Result<SpecKind, Error> {
    match x.utf8()? {
        "requires" => Ok(SpecKind::Requires),
        "ensures" => Ok(SpecKind::Ensures),
        other => Err(x.error(format!("`{other}` is not `requires` or `ensures`"))),
    }
}

/// `<int|str|bytes> <value>`: a literal a guard mentions, a bytes one as hex.
fn literal(x: At<'_>) -> Result<Literal, Error> {
    let text = x.utf8()?;
    let Some((kind, value)) = text.split_once(' ') else {
        return Err(x.error(format!("`{text}` is not `<int|str|bytes> <value>`")));
    };
    match kind {
        "int" => value
            .parse()
            .map(Literal::Int)
            .map_err(|_| x.error(format!("`{value}` is not an integer"))),
        "str" => Ok(Literal::Str(value.to_string())),
        "bytes" => unhex(value)
            .map(Literal::Bytes)
            .ok_or_else(|| x.error(format!("`{value}` is not hex"))),
        other => Err(x.error(format!("`{other}` is not `int`, `str` or `bytes`"))),
    }
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| Some(((digit(pair[0])? << 4) | digit(pair[1])?) as u8))
        .collect()
}

/// `fn <name>[ <kind>,<kind>]`, `test <name>` or `law <name>`: one keyable item of a module.
fn ordinal(x: At<'_>) -> Result<Ordinal, Error> {
    let text = x.utf8()?;
    let Some((kind, item)) = text.split_once(' ') else {
        return Err(x.error(format!("`{text}` is not `<fn|test|law> <name>`")));
    };
    Ok(match kind {
        "fn" => {
            let (name, kinds) = item.split_once(' ').unwrap_or((item, ""));
            let kinds = kinds
                .split(',')
                .filter(|k| !k.is_empty())
                .map(|k| match k {
                    "requires" => Ok(SpecKind::Requires),
                    "ensures" => Ok(SpecKind::Ensures),
                    other => Err(x.error(format!("`{other}` is not `requires` or `ensures`"))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ordinal::Fn(Symbol::new(name), kinds)
        }
        "test" => Ordinal::Test(Symbol::new(item)),
        "law" => Ordinal::Law(Symbol::new(item)),
        other => return Err(x.error(format!("`{other}` is not `fn`, `test` or `law`"))),
    })
}

struct Reader<'a> {
    sources: &'a [SourceId],
}

impl Reader<'_> {
    /// A record's `module`, `start` and `end`: a row's `At`, a label or an edit alike.
    fn span(&self, at: At<'_>) -> Result<Span, Error> {
        let index = at.field("module")?;
        let module: u32 = index.number()?;
        let source = if module == NO_MODULE {
            Span::DUMMY.source
        } else {
            *self.sources.get(module as usize).ok_or_else(|| {
                index.error(format!(
                    "a span's module, and only {} sources were handed over",
                    self.sources.len()
                ))
            })?
        };
        Ok(Span::new(
            source,
            at.field("start")?.number()?,
            at.field("end")?.number()?,
        ))
    }

    fn diagnostic(&self, d: At<'_>) -> Result<Diagnostic, Error> {
        let severity = d.field("severity")?;
        Ok(Diagnostic {
            severity: match severity.utf8()? {
                "error" => Severity::Error,
                "warning" => Severity::Warning,
                other => return Err(severity.error(format!("an unknown severity `{other}`"))),
            },
            code: intern_code(d.field("code")?.utf8()?),
            message: d.field("message")?.utf8()?.to_string(),
            labels: d.field("labels")?.items(|l| {
                Ok(Label {
                    span: self.span(l)?,
                    message: l.field("text")?.utf8()?.to_string(),
                    primary: l.field("primary")?.bool()?,
                })
            })?,
            notes: strings(d.field("notes_text")?)?,
            fixes: d.field("fixes")?.items(|f| {
                Ok(Fix {
                    title: f.field("title")?.utf8()?.to_string(),
                    edits: f.field("edits")?.items(|e| {
                        Ok(Edit {
                            span: self.span(e)?,
                            text: e.field("text")?.utf8()?.to_string(),
                        })
                    })?,
                })
            })?,
        })
    }

    fn def(&self, d: At<'_>) -> Result<(DefInfo, DefWritten), Error> {
        let info = DefInfo {
            name: Symbol::new(d.field("name")?.utf8()?),
            module: ModuleName::from_dotted(d.field("module")?.utf8()?),
            simple_name: Symbol::new(d.field("simple_name")?.utf8()?),
            footprint: footprint(d.field("footprint")?)?,
            performed: footprint(d.field("performed")?)?,
            row_aliases: symbols(d.field("row_aliases")?)?,
            spec: d.field("spec")?.items(|s| {
                Ok(SpecInfo {
                    kind: spec_kind(s.field("kind")?)?,
                    index: s.field("index")?.number()?,
                    footprint: footprint(s.field("footprint")?)?,
                    span: self.span(s.field("at")?)?,
                })
            })?,
            internally_effectful: d.field("internally_effectful")?.bool()?,
            span: self.span(d.field("at")?)?,
        };
        let written = DefWritten {
            vis: visibility(d.field("public")?.bool()?),
            reuse: d.field("reuse")?.bool()?,
            params: d.field("params")?.items(|p| {
                Ok(WrittenParam {
                    name: Symbol::new(p.field("name")?.utf8()?),
                    span: self.span(p.field("at")?)?,
                })
            })?,
            requires_literals: d.field("literals")?.items(literal)?,
        };
        Ok((info, written))
    }

    fn type_decl(&self, t: At<'_>) -> Result<TypeDecl, Error> {
        Ok(TypeDecl {
            name: Symbol::new(t.field("name")?.utf8()?),
            module: ModuleName::from_dotted(t.field("module")?.utf8()?),
            simple_name: Symbol::new(t.field("simple_name")?.utf8()?),
            vis: visibility(t.field("public")?.bool()?),
            arity: t.field("arity")?.number()?,
            span: self.span(t.field("at")?)?,
        })
    }

    /// A test's or a law's `index`, which must be its place among them.
    fn placed(&self, row: At<'_>, at: usize) -> Result<usize, Error> {
        let index = row.field("index")?;
        let i: usize = index.number()?;
        if i != at {
            return Err(index.error(format!("{i}, but the row is at {at}")));
        }
        Ok(i)
    }

    fn test(&self, t: At<'_>, at: usize) -> Result<(TestInfo, Span), Error> {
        let test = TestInfo {
            name: t.field("name")?.utf8()?.to_string(),
            module: ModuleName::from_dotted(t.field("module")?.utf8()?),
            key: Symbol::new(t.field("key")?.utf8()?),
            index: self.placed(t, at)?,
            nondet: t.field("nondet")?.bool()?,
            footprint: footprint(t.field("footprint")?)?,
            span: self.span(t.field("at")?)?,
        };
        Ok((test, self.span(t.field("name_at")?)?))
    }

    fn law(&self, l: At<'_>, at: usize) -> Result<(LawInfo, Vec<Literal>), Error> {
        let law = LawInfo {
            name: l.field("name")?.utf8()?.to_string(),
            module: ModuleName::from_dotted(l.field("module")?.utf8()?),
            key: Symbol::new(l.field("key")?.utf8()?),
            index: self.placed(l, at)?,
            has_guard: l.field("has_guard")?.bool()?,
            host: l.field("host")?.bool()?,
            footprint: footprint(l.field("footprint")?)?,
            span: self.span(l.field("at")?)?,
        };
        Ok((law, l.field("literals")?.items(literal)?))
    }

    fn effect(&self, e: At<'_>) -> Result<(EffectInfo, Visibility), Error> {
        let mut effect = EffectInfo {
            name: Symbol::new(e.field("name")?.utf8()?),
            module: ModuleName::from_dotted(e.field("module")?.utf8()?),
            simple_name: Symbol::new(e.field("simple_name")?.utf8()?),
            nondet: e.field("nondet")?.bool()?,
            ops: Default::default(),
            span: self.span(e.field("at")?)?,
        };
        for o in e.field("ops")?.list()? {
            let op = self.op(o)?;
            if effect.ops.contains_key(&op.name) {
                return Err(o.error(format!("`{}` is declared twice", op.name)));
            }
            effect.ops.insert(op.name.clone(), op);
        }
        Ok((effect, visibility(e.field("public")?.bool()?)))
    }

    fn op(&self, o: At<'_>) -> Result<OpInfo, Error> {
        let mode = o.field("mode")?;
        Ok(OpInfo {
            name: Symbol::new(o.field("name")?.utf8()?),
            mode: match mode.utf8()? {
                "read" => Mode::Read,
                "write" => Mode::Write,
                other => return Err(mode.error(format!("`{other}` is not `read` or `write`"))),
            },
            resource_param: o.field("resource_param")?.bool()?,
            span: self.span(o.field("at")?)?,
            params: published(o, "carries", carries)?.unwrap_or_default(),
        })
    }
}

// --- Carries -------------------------------------------------------------------------------

/// A row's carries. The committed bundle that stages a pull request's compiler may predate them,
/// and nothing is entered over its answer, so absent reads as `None`.
fn published<T>(
    row: At<'_>,
    field: &str,
    read: impl FnOnce(At<'_>) -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    match row.field(field) {
        Ok(x) => read(x).map(Some),
        Err(_) => Ok(None),
    }
}

fn carries(list: At<'_>) -> Result<Vec<Carry>, Error> {
    list.items(carry)
}

/// A `front.Carry`.
fn carry(c: At<'_>) -> Result<Carry, Error> {
    let k = c.ctor()?;
    Ok(match k.name() {
        "CPlain" => Carry::Plain,
        "CWidth" => {
            let n = k.arg(0)?;
            let ty = INT_TYPES
                .get(n.number::<usize>()?)
                .ok_or_else(|| n.error("a width the runtime does not number"))?;
            Carry::Width(*ty)
        }
        "CList" => Carry::List(Box::new(carry(k.arg(0)?)?)),
        "CMap" => Carry::Map(Box::new(carry(k.arg(0)?)?), Box::new(carry(k.arg(1)?)?)),
        "CRecord" => Carry::Record(k.arg(0)?.items(|f| {
            Ok((
                Symbol::new(f.field("name")?.utf8()?),
                carry(f.field("carry")?)?,
            ))
        })?),
        "CSum" => Carry::Sum(carries(k.arg(0)?)?),
        "CFn" => Carry::Fn(carries(k.arg(0)?)?, Box::new(carry(k.arg(1)?)?)),
        "CVar" => Carry::Var(k.arg(0)?.number()?),
        _ => return Err(k.unknown()),
    })
}

// --- Footprints ----------------------------------------------------------------------------

/// A footprint binds every label variable it names, numbered in the order they first appear.
fn footprint(atoms: At<'_>) -> Result<Footprint, Error> {
    let mut labels: Vec<i64> = Vec::new();
    Ok(Footprint::from_atoms(
        atoms.items(|a| atom(a, &mut labels))?,
    ))
}

/// An operation atom takes its declaration's mode, which [`resolve_op_modes`] gives it.
fn atom(a: At<'_>, labels: &mut Vec<i64>) -> Result<EffectAtom, Error> {
    let effect = Symbol::new(a.field("effect")?.utf8()?);
    let resource = resource(a.field("resource")?, labels)?;
    if let Some(op) = a.field("op")?.option()? {
        return Ok(EffectAtom::operation(
            effect,
            resource,
            Mode::Write,
            op.utf8()?,
        ));
    }
    let mode = a.field("mode")?.ctor()?;
    let mode = match mode.name() {
        "MRead" => Mode::Read,
        "MWrite" => Mode::Write,
        _ => return Err(mode.unknown()),
    };
    Ok(EffectAtom::new(effect, resource, mode))
}

fn resource(r: At<'_>, labels: &mut Vec<i64>) -> Result<Resource, Error> {
    let c = r.ctor()?;
    Ok(match c.name() {
        "RNamed" => Resource::Named(Symbol::new(c.arg(0)?.utf8()?)),
        "RSingleton" => Resource::Singleton,
        "RAny" => Resource::Every,
        "RVar" => {
            let v = c.arg(0)?.int()?;
            let at = match labels.iter().position(|held| *held == v) {
                Some(at) => at,
                None => {
                    labels.push(v);
                    labels.len() - 1
                }
            };
            Resource::Var(at as u32)
        }
        _ => return Err(c.unknown()),
    })
}

/// Every row and footprint names an operation without its mode; the declaration gives it one.
fn resolve_op_modes(front: &mut Front) {
    let modes: BTreeMap<(Symbol, Symbol), Mode> = front
        .check
        .effects
        .values()
        .flat_map(|e| {
            e.ops
                .values()
                .map(move |o| ((e.name.clone(), o.name.clone()), o.mode))
        })
        .collect();
    let mode_of = |effect: &Symbol, op: &Symbol| modes.get(&(effect.clone(), op.clone())).copied();
    for def in front.check.defs.values_mut() {
        def.footprint.resolve_modes(&mode_of);
        def.performed.resolve_modes(&mode_of);
        for spec in &mut def.spec {
            spec.footprint.resolve_modes(&mode_of);
        }
    }
    for test in &mut front.check.tests {
        test.footprint.resolve_modes(&mode_of);
    }
    for law in &mut front.check.laws {
        law.footprint.resolve_modes(&mode_of);
    }
    for sets in front.effect_sets.values_mut() {
        for set in sets {
            set.atoms.resolve_modes(&mode_of);
        }
    }
}
