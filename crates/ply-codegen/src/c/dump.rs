//! The front end's answer, a `front.Dump` as `crates/ply-compiler/ply/front.ply` builds it, read
//! into a [`Front`]. Types, rows and footprints are read from the checker's own values, numbered as
//! the printer and parser that carried them before numbered them, so an answer reads to one
//! structure however the checker happened to number its variables.

use ply_eval::Value;
use ply_eval::decode::{At, Error};
use ply_span::{Diagnostic, Edit, Fix, Label, Severity, SourceId, Span, Symbol, intern_code};
use ply_ty::front::{EmitterRoot, Pinned};
use ply_ty::{
    CtorInfo, DefConstraint, DefHash, DefInfo, DefWritten, Deriver, EffectAtom, EffectInfo,
    EffectSet, Footprint, Front, HashOutput, Hashed, LabelVar, LawBinder, LawInfo, Literal, Mode,
    ModuleInfo, ModuleName, OpInfo, Ordinal, Resource, Row, RowVar, Scheme, SpecInfo, SpecKind,
    TestInfo, TyVar, Type, TypeDecl, Visibility, WrittenParam, label_var_name,
};
use std::collections::{BTreeMap, HashMap, HashSet};

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
    for c in d.field("ctors")?.list()? {
        let ctor = r.ctor(c)?;
        front.check.ctors.insert(ctor.name.clone(), ctor);
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
            width: e.field("width")?.bool()?,
            span: r.span(e.field("at")?)?,
        })
    })?;
    front.emitter_ctors = d.field("emit_ctors")?.items(|k| {
        Ok((
            Symbol::new(k.field("name")?.utf8()?),
            k.field("arity")?.number()?,
        ))
    })?;
    front.emitter_constants = d
        .field("emit_constants")?
        .items(|c| Ok(Symbol::new(c.utf8()?)))?
        .into_iter()
        .collect();
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
            scheme: scheme(d.field("scheme")?)?,
            footprint: footprint(d.field("footprint")?)?,
            performed: footprint(d.field("performed")?)?,
            row_aliases: symbols(d.field("row_aliases")?)?,
            constraints: d.field("constraints")?.items(|c| {
                let deriver = c.field("deriver")?.ctor()?;
                Ok(DefConstraint {
                    deriver: match deriver.name() {
                        "DJson" => Deriver::Json,
                        "DEq" => Deriver::Eq,
                        "DOrd" => Deriver::Ord,
                        _ => return Err(deriver.unknown()),
                    },
                    param: c.field("param")?.number()?,
                })
            })?,
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
            binders: l.field("binders")?.items(|b| {
                Ok(LawBinder {
                    name: Symbol::new(b.field("name")?.utf8()?),
                    ty: standalone(b.field("ty")?)?,
                    span: self.span(b.field("at")?)?,
                })
            })?,
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

    /// Each parameter and the answer is a text of its own, numbered apart from the others.
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
            params: o.field("params")?.items(standalone)?,
            ret: standalone(o.field("ret")?)?,
            span: self.span(o.field("at")?)?,
            scheme: o.field("scheme")?.option()?.map(scheme).transpose()?,
        })
    }

    fn ctor(&self, k: At<'_>) -> Result<CtorInfo, Error> {
        let scheme = scheme(k.field("scheme")?)?;
        // From the scheme, so the fields share its numbering.
        let fields: Vec<Type> = match &scheme.ty {
            Type::Fn { params, .. } => params.clone(),
            _ => Vec::new(),
        };
        let written = k.field("fields")?;
        if written.list()?.len() != fields.len() {
            return Err(written.error(format!(
                "{} field(s), and {} in the scheme",
                written.list()?.len(),
                fields.len()
            )));
        }
        Ok(CtorInfo {
            name: Symbol::new(k.field("name")?.utf8()?),
            module: ModuleName::from_dotted(k.field("module")?.utf8()?),
            simple_name: Symbol::new(k.field("simple_name")?.utf8()?),
            type_name: Symbol::new(k.field("type_name")?.utf8()?),
            index: k.field("index")?.number()?,
            arity: k.field("arity")?.number()?,
            fields,
            scheme,
            span: self.span(k.field("at")?)?,
        })
    }
}

// --- Types ---------------------------------------------------------------------------------

/// A scheme's head binds its variables in order; the rest are numbered where they first appear.
fn scheme(s: At<'_>) -> Result<Scheme, Error> {
    let ty = s.field("ty")?;
    let mut n = Numbering::of_text(ty)?;
    let ty_vars = s.field("ty_vars")?.items(|v| Ok(n.ty(v.int()?)))?;
    let label_vars = s.field("label_vars")?.items(|v| Ok(n.bind(v.int()?)))?;
    let row_vars = s.field("row_vars")?.items(|v| Ok(n.row(v.int()?)))?;
    Ok(Scheme {
        ty_vars,
        row_vars,
        label_vars,
        ty: n.ty_of(ty)?,
    })
}

/// A type with no head of its own, as an operation's parameter or a law's binder is.
fn standalone(t: At<'_>) -> Result<Type, Error> {
    Numbering::of_text(t)?.ty_of(t)
}

/// A footprint binds every label variable it names, in the order they first appear.
fn footprint(atoms: At<'_>) -> Result<Footprint, Error> {
    let mut n = Numbering {
        binds_on_sight: true,
        ..Numbering::default()
    };
    Ok(Footprint::from_atoms(atoms.items(|a| n.atom(a))?))
}

/// The variables of one text, numbered as a reader of it numbers them. A label variable the text
/// does not bind reads back as a resource under the name the printer gave it.
#[derive(Default)]
struct Numbering {
    tys: HashMap<i64, u32>,
    /// Counts the region variable each cell without a named region reads back with, too.
    next_ty: u32,
    rows: HashMap<i64, u32>,
    bound: HashMap<i64, u32>,
    binds_on_sight: bool,
    /// Every label variable's name, bound or not, which no later one may take.
    names: HashMap<i64, String>,
    /// The resources the text names, which no label variable may take.
    taken: HashSet<String>,
}

impl Numbering {
    fn of_text(t: At<'_>) -> Result<Numbering, Error> {
        let mut n = Numbering::default();
        n.reserve(t)?;
        Ok(n)
    }

    fn reserve(&mut self, t: At<'_>) -> Result<(), Error> {
        let c = t.ctor()?;
        match c.name() {
            "TyVar" => {}
            "TyCon" => {
                for a in c.arg(0)?.field("args")?.list()? {
                    self.reserve(a)?;
                }
            }
            "TyFn" => {
                let f = c.arg(0)?;
                for p in f.field("params")?.list()? {
                    self.reserve(p)?;
                }
                self.reserve(f.field("ret")?)?;
                for a in f.field("effects")?.field("atoms")?.list()? {
                    let r = a.field("resource")?.ctor()?;
                    if r.name() == "RNamed" {
                        self.taken.insert(r.arg(0)?.utf8()?.to_string());
                    }
                }
            }
            "TyRecord" => {
                for f in c.arg(0)?.list()? {
                    self.reserve(f.field("ty")?)?;
                }
            }
            _ => return Err(c.unknown()),
        }
        Ok(())
    }

    fn ty(&mut self, v: i64) -> TyVar {
        if let Some(&n) = self.tys.get(&v) {
            return TyVar(n);
        }
        let n = self.fresh();
        self.tys.insert(v, n.0);
        n
    }

    fn fresh(&mut self) -> TyVar {
        let n = TyVar(self.next_ty);
        self.next_ty += 1;
        n
    }

    fn row(&mut self, v: i64) -> RowVar {
        let next = self.rows.len() as u32;
        RowVar(*self.rows.entry(v).or_insert(next))
    }

    fn bind(&mut self, v: i64) -> LabelVar {
        let next = self.bound.len() as u32;
        let bound = LabelVar(*self.bound.entry(v).or_insert(next));
        self.name(v);
        bound
    }

    /// The first label letter, then round, that no resource and no other label here holds.
    fn name(&mut self, v: i64) -> String {
        if let Some(name) = self.names.get(&v) {
            return name.clone();
        }
        let name = (0..)
            .map(|i| label_var_name(LabelVar(i)))
            .find(|n| !self.taken.contains(n) && !self.names.values().any(|held| held == n))
            .expect("the letters and their rounds do not run out");
        self.names.insert(v, name.clone());
        name
    }

    fn ty_of(&mut self, t: At<'_>) -> Result<Type, Error> {
        let c = t.ctor()?;
        Ok(match c.name() {
            "TyVar" => Type::Var(self.ty(c.arg(0)?.int()?)),
            "TyCon" => {
                let con = c.arg(0)?;
                let name = con.field("name")?.utf8()?;
                let args: Vec<At<'_>> = con.field("args")?.list()?.collect();
                let unnamed_cell = name == "Cell"
                    && match args[..] {
                        [_] => true,
                        [region, _] => !is_region(region)?,
                        _ => false,
                    };
                if unnamed_cell {
                    // A cell prints its region only when it is a named one; otherwise it reads back
                    // with a variable of its own, placed after its element's.
                    let elem = self.ty_of(args[args.len() - 1])?;
                    let region = Type::Var(self.fresh());
                    Type::Con(Symbol::new(name), vec![region, elem])
                } else {
                    let args = args
                        .into_iter()
                        .map(|a| self.ty_of(a))
                        .collect::<Result<_, _>>()?;
                    Type::Con(Symbol::new(name), args)
                }
            }
            "TyFn" => {
                let f = c.arg(0)?;
                let params = f.field("params")?.items(|p| self.ty_of(p))?;
                let ret = self.ty_of(f.field("ret")?)?;
                let effects = self.row_of(f.field("effects")?)?;
                Type::Fn {
                    params,
                    ret: Box::new(ret),
                    effects,
                }
            }
            "TyRecord" => {
                let list = c.arg(0)?;
                let fields: Vec<(&str, At<'_>)> =
                    list.items(|f| Ok((f.field("name")?.utf8()?, f.field("ty")?)))?;
                // A tuple prints its items by position, and they are read back in that order.
                let position = |i: usize| {
                    let name = format!("_{i}");
                    fields.iter().position(|(n, _)| *n == name)
                };
                let order: Vec<usize> =
                    if fields.len() >= 2 && (0..fields.len()).all(|i| position(i).is_some()) {
                        (0..fields.len()).filter_map(position).collect()
                    } else {
                        (0..fields.len()).collect()
                    };
                let mut out = BTreeMap::new();
                for i in order {
                    let (name, ty) = fields[i];
                    if out.insert(Symbol::new(name), self.ty_of(ty)?).is_some() {
                        return Err(list.error(format!("the field `{name}` twice")));
                    }
                }
                Type::Record(out)
            }
            _ => return Err(c.unknown()),
        })
    }

    fn row_of(&mut self, r: At<'_>) -> Result<Row, Error> {
        let mut row = Row::empty();
        for a in r.field("atoms")?.list()? {
            row.atoms.insert(self.atom(a)?);
        }
        row.tail = match r.field("tail")?.option()? {
            Some(v) => Some(self.row(v.int()?)),
            None => None,
        };
        Ok(row)
    }

    /// An operation atom takes its declaration's mode, which [`resolve_op_modes`] gives it.
    fn atom(&mut self, a: At<'_>) -> Result<EffectAtom, Error> {
        let effect = Symbol::new(a.field("effect")?.utf8()?);
        let resource = self.resource(a.field("resource")?)?;
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

    fn resource(&mut self, r: At<'_>) -> Result<Resource, Error> {
        let c = r.ctor()?;
        Ok(match c.name() {
            "RNamed" => Resource::Named(Symbol::new(c.arg(0)?.utf8()?)),
            "RSingleton" => Resource::Singleton,
            "RAny" => Resource::Every,
            "RVar" => {
                let v = c.arg(0)?.int()?;
                match self.bound.get(&v) {
                    Some(&bound) => Resource::Var(LabelVar(bound)),
                    None if self.binds_on_sight => Resource::Var(self.bind(v)),
                    None => Resource::Named(Symbol::new(self.name(v))),
                }
            }
            _ => return Err(c.unknown()),
        })
    }
}

/// A named region: a constructor of no arguments under the region prefix.
fn is_region(t: At<'_>) -> Result<bool, Error> {
    let c = t.ctor()?;
    if c.name() != "TyCon" {
        return Ok(false);
    }
    let con = c.arg(0)?;
    Ok(con.field("args")?.list()?.next().is_none()
        && con
            .field("name")?
            .utf8()?
            .starts_with(ply_ty::print::REGION_PREFIX))
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
        def.scheme.ty.resolve_modes(&mode_of);
        for spec in &mut def.spec {
            spec.footprint.resolve_modes(&mode_of);
        }
    }
    for test in &mut front.check.tests {
        test.footprint.resolve_modes(&mode_of);
    }
    for law in &mut front.check.laws {
        law.footprint.resolve_modes(&mode_of);
        for binder in &mut law.binders {
            binder.ty.resolve_modes(&mode_of);
        }
    }
    for ctor in front.check.ctors.values_mut() {
        ctor.scheme.ty.resolve_modes(&mode_of);
        for field in &mut ctor.fields {
            field.resolve_modes(&mode_of);
        }
    }
    for effect in front.check.effects.values_mut() {
        for op in effect.ops.values_mut() {
            for param in &mut op.params {
                param.resolve_modes(&mode_of);
            }
            op.ret.resolve_modes(&mode_of);
            if let Some(scheme) = &mut op.scheme {
                scheme.ty.resolve_modes(&mode_of);
            }
        }
    }
    for sets in front.effect_sets.values_mut() {
        for set in sets {
            set.atoms.resolve_modes(&mode_of);
        }
    }
}
