//! The front end's whole answer over a program as length-framed text — the diagnostics, the load
//! order, the checker's output, the hashes, the item ordinals and the stored bodies — read back
//! into the structs the driver, the store and the backend consume, and written from them. The
//! writer is the protocol: the self-hosted front end's `front.front_dump` is held to its text, and
//! `crates/ply-compiler/ply/front.ply` writes the same grammar.
//!
//! A frame is `<kind> <name> <length>\n<payload>`; a payload is a run of fields, each
//! `<key> <length>\n<bytes>`, unless the kind says otherwise. Every length is in bytes. Frames come
//! in this order, each kind in the order its source table iterates:
//!
//! ```text
//! diag <i> <n>              exactly ply_span::frames; if any is an error the dump ends here
//! order _ <n>               module <n>\n<module name>                    dependency-first
//! module <name> <n>         index <n>\n<position in the source list>      in source-list order
//!                           item <n>\n<program-wide name>                  ModuleInfo::items
//!                           import <n>\n<module name>
//! def <name> <n>            module, simple_name, scheme (print_scheme), footprint, performed
//!                           constraint <n>\n<deriver> <ty_vars index>
//!                           internally_effectful <n>\n0|1
//!                           row_alias <n>\n<effect set's simple name>     source order
//!                           spec <n>\n<requires|ensures> <index> <footprint>\n<span>
//!                           span <n>\n<span>
//! test <i> <n>              key, name, module, index, nondet 0|1, footprint, span
//! law <i> <n>               key, name, module, index
//!                           binder <n>\n<name> <span>\n<print_type>
//!                           has_guard 0|1, host 0|1, footprint, span
//! effect <name> <n>         module, simple_name, nondet 0|1
//!                           op <n>\n<name> <read|write> <resource_param 0|1> <param count>
//!                                        <has_scheme 0|1> <span>\n
//!                                 <print_type>\n        one line per parameter
//!                                 <print_type>\n        the result
//!                                 <print_scheme>\n      only when has_scheme is 1
//!                           span
//! ctor <name> <n>           module, simple_name, type_name, index, arity
//!                           field <n>\n<print_type>
//!                           scheme, span
//! hash <name> <n>           def <n>\n<hex>, own <n>\n<hex>, decl <n>\n<hex>   each when present
//!                           spec <n>\n<hex>, spec_text <n>\n<hex>           clause order
//!                           dep <n>\n<name>                                 reference order
//!                           closure <n>\n<name>                             sorted
//! testhash <i> <n>          key, hash, dep*, closure*
//! lawhash <i> <n>           key, hash, text, dep*, closure*
//! ordinal <module> <n>      item <n>\n<fn <name>[ <kind>,<kind>..] | test <key> | law <key>>
//! body <name> <n>           a fn's, type's or effect's stored body as lowercase hex; no fields
//! testbody <i> <n>          a test's, likewise
//! ```
//!
//! A footprint is `atom,atom` with no spaces, empty for the pure one; an atom is
//! `effect.mode[resource]`, the resource omitted for a singleton. A span is `<module> <start>
//! <end>`, the module its position in the source list and `4294967295` outside every module. A
//! module name is its dotted form, empty for the anonymous module. Tests and laws are numbered by
//! their position in `CheckOutput::tests` and `::laws`, which is the position their hashes hold,
//! and the `test` and `law` frames come in that order. The `hash`, `testhash` and `lawhash`
//! frames follow the hasher's item order — every module in program order, its items in source
//! order, a name declared in two namespaces once — which [`Front::hash_order`] records, so a
//! `testhash` may sit between two `hash` frames; the reader takes them in any order by their
//! index and requires each index once. The bodies follow the same walk, one per declaration, so
//! a name in two namespaces has two. A fn's ordinal names its clauses' kinds in source order, so
//! a clause root can be numbered without the source.

use crate::hash::{DefHash, HashOutput};
use crate::parse::{parse_footprint, parse_scheme, parse_type};
use crate::print::{print_scheme, print_type};
use crate::{
    CheckOutput, CtorInfo, DefConstraint, DefInfo, Deriver, EffectInfo, Footprint, LawBinder,
    LawInfo, Mode, ModuleInfo, ModuleName, OpInfo, Scheme, SpecInfo, SpecKind, TestInfo, Type,
};
use indexmap::IndexMap;
use ply_span::frames::{Cursor, read_diagnostics, write_diagnostics};
use ply_span::{Diagnostic, Severity, SourceId, Span, Symbol};
use std::collections::BTreeSet;

/// The module index of a span outside every module.
const NO_MODULE: u32 = u32::MAX;

/// One keyable item of a module, in source order: what the backend's cache keys are minted from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ordinal {
    /// A `fn`, by program-wide name, with the kind of each `requires` / `ensures` clause in
    /// source order.
    Fn(Symbol, Vec<SpecKind>),
    /// A `test`, by `<module>.<label>`.
    Test(Symbol),
    /// A `law`, by `<module>.<label>`.
    Law(Symbol),
}

/// One entry of the hasher's item order: what a `hash`, `testhash` or `lawhash` frame is about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Hashed {
    /// A `fn`, `type` or `effect`, by program-wide name; one entry for a name in two namespaces.
    Def(Symbol),
    /// A test, by its position in `CheckOutput::tests`.
    Test(usize),
    /// A law, by its position in `CheckOutput::laws`.
    Law(usize),
}

#[derive(Clone, Debug, Default)]
pub struct Front {
    pub diagnostics: Vec<Diagnostic>,
    /// Dependency-first module order, by module name.
    pub order: Vec<Symbol>,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// The hasher's item order: every hashed name, test and law, as the `hash` frames are written.
    pub hash_order: Vec<Hashed>,
    /// Per module in program order, its keyable items in source order.
    pub ordinals: Vec<(Symbol, Vec<Ordinal>)>,
    /// Every `fn`'s, `type`'s and `effect`'s stored body, by program-wide name, in the hasher's
    /// item order.
    pub bodies: Vec<(Symbol, Vec<u8>)>,
    /// Parallel to `CheckOutput::tests`.
    pub test_bodies: Vec<Vec<u8>>,
}

impl Front {
    pub fn has_error(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

// --- writing -----------------------------------------------------------------

pub fn write_front(front: &Front, sources: &[SourceId]) -> Result<String, String> {
    let mut out = write_diagnostics(&front.diagnostics, sources)?;
    if front.has_error() {
        return Ok(out);
    }
    let w = Writer { sources };

    let mut p = Payload::default();
    for m in &front.order {
        p.field("module", m.as_str());
    }
    p.frame(&mut out, "order", "_");

    for (name, m) in &front.check.modules {
        let mut p = Payload::default();
        let index = sources.iter().position(|s| *s == m.source).ok_or_else(|| {
            format!(
                "module `{name}` is source {}, which is not among the {} sources handed over",
                m.source.0,
                sources.len()
            )
        })?;
        p.field("index", &index.to_string());
        for item in &m.items {
            p.field("item", item.as_str());
        }
        for import in &m.imports {
            p.field("import", import.as_str());
        }
        p.frame(&mut out, "module", name.as_str());
    }

    for (name, d) in &front.check.defs {
        let what = format!("def `{name}`");
        let mut p = Payload::default();
        p.field("module", d.module.as_str());
        p.field("simple_name", d.simple_name.as_str());
        p.field("scheme", &print_scheme(&d.scheme));
        p.field("footprint", &footprint_text(&d.footprint));
        p.field("performed", &footprint_text(&d.performed));
        for c in &d.constraints {
            p.field("constraint", &format!("{} {}", c.deriver, c.param));
        }
        p.field("internally_effectful", flag(d.internally_effectful));
        for a in &d.row_aliases {
            p.field("row_alias", a.as_str());
        }
        for s in &d.spec {
            p.field(
                "spec",
                &format!(
                    "{} {} {}\n{}",
                    s.kind.as_str(),
                    s.index,
                    footprint_text(&s.footprint),
                    w.span(s.span, &what)?
                ),
            );
        }
        p.field("span", &w.span(d.span, &what)?);
        p.frame(&mut out, "def", name.as_str());
    }

    for (i, t) in front.check.tests.iter().enumerate() {
        let what = format!("test {i}");
        let mut p = Payload::default();
        p.field("key", t.key.as_str());
        p.field("name", &t.name);
        p.field("module", t.module.as_str());
        p.field("index", &t.index.to_string());
        p.field("nondet", flag(t.nondet));
        p.field("footprint", &footprint_text(&t.footprint));
        p.field("span", &w.span(t.span, &what)?);
        p.frame(&mut out, "test", &i.to_string());
    }

    for (i, l) in front.check.laws.iter().enumerate() {
        let what = format!("law {i}");
        let mut p = Payload::default();
        p.field("key", l.key.as_str());
        p.field("name", &l.name);
        p.field("module", l.module.as_str());
        p.field("index", &l.index.to_string());
        for b in &l.binders {
            p.field(
                "binder",
                &format!(
                    "{} {}\n{}",
                    b.name,
                    w.span(b.span, &what)?,
                    print_type(&b.ty)
                ),
            );
        }
        p.field("has_guard", flag(l.has_guard));
        p.field("host", flag(l.host));
        p.field("footprint", &footprint_text(&l.footprint));
        p.field("span", &w.span(l.span, &what)?);
        p.frame(&mut out, "law", &i.to_string());
    }

    for (name, e) in &front.check.effects {
        let what = format!("effect `{name}`");
        let mut p = Payload::default();
        p.field("module", e.module.as_str());
        p.field("simple_name", e.simple_name.as_str());
        p.field("nondet", flag(e.nondet));
        for o in e.ops.values() {
            let mut text = format!(
                "{} {} {} {} {} {}\n",
                o.name,
                o.mode.as_str(),
                flag(o.resource_param),
                o.params.len(),
                flag(o.scheme.is_some()),
                w.span(o.span, &what)?
            );
            for param in &o.params {
                text.push_str(&print_type(param));
                text.push('\n');
            }
            text.push_str(&print_type(&o.ret));
            text.push('\n');
            if let Some(scheme) = &o.scheme {
                text.push_str(&print_scheme(scheme));
                text.push('\n');
            }
            p.field("op", &text);
        }
        p.field("span", &w.span(e.span, &what)?);
        p.frame(&mut out, "effect", name.as_str());
    }

    for (name, c) in &front.check.ctors {
        let what = format!("ctor `{name}`");
        let mut p = Payload::default();
        p.field("module", c.module.as_str());
        p.field("simple_name", c.simple_name.as_str());
        p.field("type_name", c.type_name.as_str());
        p.field("index", &c.index.to_string());
        p.field("arity", &c.arity.to_string());
        for f in &c.fields {
            p.field("field", &print_type(f));
        }
        p.field("scheme", &print_scheme(&c.scheme));
        p.field("span", &w.span(c.span, &what)?);
        p.frame(&mut out, "ctor", name.as_str());
    }

    write_hashes(front, &mut out)?;

    for (module, items) in &front.ordinals {
        let mut p = Payload::default();
        for item in items {
            let text = match item {
                Ordinal::Fn(name, kinds) if kinds.is_empty() => format!("fn {name}"),
                Ordinal::Fn(name, kinds) => {
                    let kinds: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
                    format!("fn {name} {}", kinds.join(","))
                }
                Ordinal::Test(key) => format!("test {key}"),
                Ordinal::Law(key) => format!("law {key}"),
            };
            p.field("item", &text);
        }
        p.frame(&mut out, "ordinal", module.as_str());
    }

    for (name, bytes) in &front.bodies {
        raw_frame(&mut out, "body", name.as_str(), &hex(bytes));
    }
    for (i, bytes) in front.test_bodies.iter().enumerate() {
        raw_frame(&mut out, "testbody", &i.to_string(), &hex(bytes));
    }
    Ok(out)
}

/// One frame per entry of [`Front::hash_order`], each carrying what every map of `HashOutput`
/// holds under its key; a test whose key is also a definition's name shares the reach that key
/// holds, as the hasher merged it.
fn write_hashes(front: &Front, out: &mut String) -> Result<(), String> {
    let h = &front.hashes;
    for (table, keys) in [
        ("defs", h.defs.keys().collect::<Vec<_>>()),
        ("own", h.own.keys().collect()),
        ("decls", h.decls.keys().collect()),
        ("specs", h.specs.keys().collect()),
        ("spec_texts", h.spec_texts.keys().collect()),
        ("closure", h.closure.keys().collect()),
    ] {
        if let Some(key) = keys.into_iter().find(|k| !h.deps.contains_key(*k)) {
            return Err(format!(
                "`{key}` is in the hashes' `{table}` but has no `deps` entry, which every hashed \
                 name has"
            ));
        }
    }
    let parallel = |what: &str, len: usize, of: &str, want: usize| {
        if len == want {
            Ok(())
        } else {
            Err(format!("{len} {what} hashes beside {want} {of}"))
        }
    };
    parallel("test", h.tests.len(), "tests", front.check.tests.len())?;
    parallel("law", h.laws.len(), "laws", front.check.laws.len())?;
    parallel(
        "law text",
        h.law_texts.len(),
        "laws",
        front.check.laws.len(),
    )?;

    let reach = |p: &mut Payload, name: &Symbol| {
        for d in h.deps.get(name).into_iter().flatten() {
            p.field("dep", d.as_str());
        }
        for c in h.closure.get(name).into_iter().flatten() {
            p.field("closure", c.as_str());
        }
    };
    let mut named: BTreeSet<&Symbol> = BTreeSet::new();
    let mut tests: BTreeSet<usize> = BTreeSet::new();
    let mut laws: BTreeSet<usize> = BTreeSet::new();
    for entry in &front.hash_order {
        match entry {
            Hashed::Def(name) => {
                if !h.deps.contains_key(name) {
                    return Err(format!(
                        "the hash order names `{name}`, which was not hashed"
                    ));
                }
                if !named.insert(name) {
                    return Err(format!("the hash order names `{name}` twice"));
                }
                let mut p = Payload::default();
                if let Some(hash) = h.defs.get(name) {
                    p.field("def", &hash.to_hex());
                }
                if let Some(hash) = h.own.get(name) {
                    p.field("own", &hash.to_hex());
                }
                if let Some(hash) = h.decls.get(name) {
                    p.field("decl", &hash.to_hex());
                }
                for hash in h.specs.get(name).into_iter().flatten() {
                    p.field("spec", &hash.to_hex());
                }
                for hash in h.spec_texts.get(name).into_iter().flatten() {
                    p.field("spec_text", &hash.to_hex());
                }
                reach(&mut p, name);
                p.frame(out, "hash", name.as_str());
            }
            Hashed::Test(i) => {
                let t =
                    front.check.tests.get(*i).ok_or_else(|| {
                        format!("the hash order names test {i}, which there is not")
                    })?;
                if !tests.insert(*i) {
                    return Err(format!("the hash order names test {i} twice"));
                }
                let mut p = Payload::default();
                p.field("key", t.key.as_str());
                p.field("hash", &h.tests[*i].to_hex());
                reach(&mut p, &t.key);
                p.frame(out, "testhash", &i.to_string());
            }
            Hashed::Law(i) => {
                let l =
                    front.check.laws.get(*i).ok_or_else(|| {
                        format!("the hash order names law {i}, which there is not")
                    })?;
                if !laws.insert(*i) {
                    return Err(format!("the hash order names law {i} twice"));
                }
                let mut p = Payload::default();
                p.field("key", l.key.as_str());
                p.field("hash", &h.laws[*i].to_hex());
                p.field("text", &h.law_texts[*i].to_hex());
                reach(&mut p, &l.key);
                p.frame(out, "lawhash", &i.to_string());
            }
        }
    }
    for name in h.defs.keys().chain(h.decls.keys()).chain(h.own.keys()) {
        if !named.contains(name) {
            return Err(format!(
                "`{name}` is hashed but the hash order never names it"
            ));
        }
    }
    if tests.len() != front.check.tests.len() || laws.len() != front.check.laws.len() {
        return Err(format!(
            "the hash order names {} of {} tests and {} of {} laws",
            tests.len(),
            front.check.tests.len(),
            laws.len(),
            front.check.laws.len()
        ));
    }
    Ok(())
}

struct Writer<'a> {
    sources: &'a [SourceId],
}

impl Writer<'_> {
    fn span(&self, span: Span, what: &str) -> Result<String, String> {
        let module = if span.is_dummy() {
            NO_MODULE
        } else {
            self.sources
                .iter()
                .position(|s| *s == span.source)
                .map(|i| i as u32)
                .ok_or_else(|| {
                    format!(
                        "{what} spans source {}, which is not among the {} sources handed over",
                        span.source.0,
                        self.sources.len()
                    )
                })?
        };
        Ok(format!("{module} {} {}", span.start, span.end))
    }
}

#[derive(Default)]
struct Payload(String);

impl Payload {
    fn field(&mut self, key: &str, text: &str) {
        self.0.push_str(&format!("{key} {}\n{text}", text.len()));
    }

    fn frame(self, out: &mut String, kind: &str, name: &str) {
        raw_frame(out, kind, name, &self.0);
    }
}

fn raw_frame(out: &mut String, kind: &str, name: &str, payload: &str) {
    out.push_str(&format!("{kind} {name} {}\n{payload}", payload.len()));
}

fn flag(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}

fn footprint_text(f: &Footprint) -> String {
    f.atoms()
        .map(|a| a.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
    }
    s
}

// --- reading -----------------------------------------------------------------

/// A frame kind, a field key or a value the protocol does not know is an error naming it, and so is
/// a frame or a field that ends before its length says, a required field that is missing, and a
/// frame that names a test or a law no earlier frame declared.
pub fn read_front(dump: &str, sources: &[SourceId]) -> Result<Front, String> {
    let mut frames = Cursor::new(dump.as_bytes(), "frame");
    let mut front = Front::default();
    let r = Reader { sources };
    // Filled by index, since the hasher's order interleaves these with the `hash` frames.
    let mut test_hashes: Vec<Option<DefHash>> = Vec::new();
    let mut law_hashes: Vec<Option<(DefHash, DefHash)>> = Vec::new();

    // The diagnostics lead, and `ply_span::frames` reads them as one text.
    let mut diag_end = 0;
    let mut leading = true;
    let mut index = 0;
    while !frames.done() {
        let (words, payload) = frames.unit()?;
        if leading && words.first() == Some(&"diag") {
            diag_end = frames.at();
            index += 1;
            continue;
        }
        if leading {
            leading = false;
            front.diagnostics = read_diagnostics(&dump[..diag_end], sources)?;
            if front.has_error() {
                return Err(
                    "the dump continues past an error diagnostic, where the front end has no \
                     answer"
                        .to_string(),
                );
            }
        }
        let [kind, name] = words[..] else {
            return Err(format!(
                "frame {index}: header `{}` is not `<kind> <name> <length>`",
                words.join(" ")
            ));
        };
        let what = format!("{kind} `{name}`");
        match kind {
            "diag" => {
                return Err(format!(
                    "frame {index}: a `diag` frame after the diagnostics, which lead the dump"
                ));
            }
            "order" => {
                for (key, text) in Fields::of(payload, &what)?.all() {
                    match key {
                        "module" => front.order.push(Symbol::new(text)),
                        other => return Err(unknown_field(&what, other)),
                    }
                }
            }
            "module" => {
                let fields = Fields::of(payload, &what)?;
                let mut source = None;
                let (mut items, mut imports) = (Vec::new(), Vec::new());
                for (key, text) in fields.all() {
                    match key {
                        "index" => fields.once(&mut source, key, text)?,
                        "item" => items.push(Symbol::new(text)),
                        "import" => imports.push(ModuleName::from_dotted(text)),
                        other => return Err(unknown_field(&what, other)),
                    }
                }
                let source: usize = fields.number(fields.required(source, "index")?, "index")?;
                let source = *sources.get(source).ok_or_else(|| {
                    format!(
                        "{what} is source {source}, and only {} sources were handed over",
                        sources.len()
                    )
                })?;
                front.check.modules.insert(
                    Symbol::new(name),
                    ModuleInfo {
                        name: ModuleName::from_dotted(name),
                        source,
                        items,
                        imports,
                    },
                );
            }
            "def" => {
                let def = r.def(name, payload, &what)?;
                front.check.defs.insert(Symbol::new(name), def);
            }
            "test" => {
                r.numbered(name, front.check.tests.len(), &what)?;
                let test = r.test(front.check.tests.len(), payload, &what)?;
                front.check.tests.push(test);
            }
            "law" => {
                r.numbered(name, front.check.laws.len(), &what)?;
                let law = r.law(front.check.laws.len(), payload, &what)?;
                front.check.laws.push(law);
            }
            "effect" => {
                let effect = r.effect(name, payload, &what)?;
                front.check.effects.insert(Symbol::new(name), effect);
            }
            "ctor" => {
                let ctor = r.ctor(name, payload, &what)?;
                front.check.ctors.insert(Symbol::new(name), ctor);
            }
            "hash" => {
                r.hash(name, payload, &what, &mut front.hashes)?;
                front.hash_order.push(Hashed::Def(Symbol::new(name)));
            }
            "testhash" => {
                let i = r.item_index(name, front.check.tests.len(), "test", &what)?;
                let key = &front.check.tests[i].key;
                let (hash, _) = r.item_hash(payload, &what, key, false, &mut front.hashes)?;
                test_hashes.resize(front.check.tests.len(), None);
                if test_hashes[i].replace(hash).is_some() {
                    return Err(format!("{what} is written twice"));
                }
                front.hash_order.push(Hashed::Test(i));
            }
            "lawhash" => {
                let i = r.item_index(name, front.check.laws.len(), "law", &what)?;
                let key = &front.check.laws[i].key;
                let (hash, text) = r.item_hash(payload, &what, key, true, &mut front.hashes)?;
                let text = text.ok_or_else(|| format!("{what} has no `text`"))?;
                law_hashes.resize(front.check.laws.len(), None);
                if law_hashes[i].replace((hash, text)).is_some() {
                    return Err(format!("{what} is written twice"));
                }
                front.hash_order.push(Hashed::Law(i));
            }
            "ordinal" => {
                let mut items = Vec::new();
                for (key, text) in Fields::of(payload, &what)?.all() {
                    if key != "item" {
                        return Err(unknown_field(&what, key));
                    }
                    items.push(ordinal(text, &what)?);
                }
                front.ordinals.push((Symbol::new(name), items));
            }
            "body" => front
                .bodies
                .push((Symbol::new(name), unhex(payload, &what)?)),
            "testbody" => {
                r.numbered(name, front.test_bodies.len(), &what)?;
                front.test_bodies.push(unhex(payload, &what)?);
            }
            other => return Err(format!("frame {index}: unknown frame kind `{other}`")),
        }
        index += 1;
    }
    if leading {
        front.diagnostics = read_diagnostics(&dump[..diag_end], sources)?;
        if !front.has_error() {
            return Err(
                "the dump ends after the diagnostics, none of which is an error".to_string(),
            );
        }
        return Ok(front);
    }

    test_hashes.resize(front.check.tests.len(), None);
    law_hashes.resize(front.check.laws.len(), None);
    for (i, hash) in test_hashes.into_iter().enumerate() {
        front
            .hashes
            .tests
            .push(hash.ok_or_else(|| format!("test {i} has no `testhash` frame"))?);
    }
    for (i, hashes) in law_hashes.into_iter().enumerate() {
        let (hash, text) = hashes.ok_or_else(|| format!("law {i} has no `lawhash` frame"))?;
        front.hashes.laws.push(hash);
        front.hashes.law_texts.push(text);
    }
    if front.test_bodies.len() != front.check.tests.len() {
        return Err(format!(
            "{} testbody frames beside {} tests",
            front.test_bodies.len(),
            front.check.tests.len()
        ));
    }
    Ok(front)
}

fn unknown_field(what: &str, key: &str) -> String {
    format!("{what}: unknown field `{key}`")
}

fn spec_kind(text: &str, what: &str) -> Result<SpecKind, String> {
    match text {
        "requires" => Ok(SpecKind::Requires),
        "ensures" => Ok(SpecKind::Ensures),
        other => Err(format!("{what}: `{other}` is not `requires` or `ensures`")),
    }
}

fn ordinal(text: &str, what: &str) -> Result<Ordinal, String> {
    let Some((kind, item)) = text.split_once(' ') else {
        return Err(format!(
            "{what}: item `{text}` is not `<fn|test|law> <name>`"
        ));
    };
    Ok(match kind {
        "fn" => {
            let (name, kinds) = item.split_once(' ').unwrap_or((item, ""));
            let kinds = kinds
                .split(',')
                .filter(|k| !k.is_empty())
                .map(|k| spec_kind(k, what))
                .collect::<Result<Vec<_>, _>>()?;
            Ordinal::Fn(Symbol::new(name), kinds)
        }
        "test" => Ordinal::Test(Symbol::new(item)),
        "law" => Ordinal::Law(Symbol::new(item)),
        other => {
            return Err(format!("{what}: `{other}` is not `fn`, `test` or `law`"));
        }
    })
}

fn unhex(payload: &[u8], what: &str) -> Result<Vec<u8>, String> {
    if !payload.len().is_multiple_of(2) {
        return Err(format!("{what}: an odd number of hex digits"));
    }
    let digit = |b: u8| {
        (b as char)
            .to_digit(16)
            .ok_or_else(|| format!("{what}: `{}` is not a hex digit", b as char))
    };
    payload
        .chunks_exact(2)
        .map(|pair| Ok(((digit(pair[0])? << 4) | digit(pair[1])?) as u8))
        .collect()
}

/// The fields of one frame's payload, checked and read in order.
struct Fields<'a> {
    pairs: Vec<(&'a str, &'a str)>,
    what: &'a str,
}

impl<'a> Fields<'a> {
    fn of(payload: &'a [u8], what: &'a str) -> Result<Self, String> {
        let mut cursor = Cursor::new(payload, "field");
        let mut pairs = Vec::new();
        while !cursor.done() {
            let (words, body) = cursor.unit()?;
            let [key] = words[..] else {
                return Err(format!(
                    "{what}: field header `{}` is not `<key> <length>`",
                    words.join(" ")
                ));
            };
            let text = std::str::from_utf8(body)
                .map_err(|e| format!("{what}: `{key}` is not UTF-8: {e}"))?;
            pairs.push((key, text));
        }
        Ok(Fields { pairs, what })
    }

    fn all(&self) -> Vec<(&'a str, &'a str)> {
        self.pairs.clone()
    }

    fn once(&self, slot: &mut Option<&'a str>, key: &str, text: &'a str) -> Result<(), String> {
        if slot.replace(text).is_some() {
            return Err(format!("{} has two `{key}` fields", self.what));
        }
        Ok(())
    }

    fn required(&self, slot: Option<&'a str>, key: &str) -> Result<&'a str, String> {
        slot.ok_or_else(|| format!("{} has no `{key}`", self.what))
    }

    fn number<N: std::str::FromStr>(&self, text: &str, key: &str) -> Result<N, String> {
        text.parse()
            .map_err(|_| format!("{}: `{key}` holds `{text}`, not a number", self.what))
    }

    fn flag(&self, text: &str, key: &str) -> Result<bool, String> {
        match text {
            "0" => Ok(false),
            "1" => Ok(true),
            other => Err(format!("{}: `{key}` is `{other}`, not 0 or 1", self.what)),
        }
    }

    fn footprint(&self, text: &str, key: &str) -> Result<Footprint, String> {
        parse_footprint(text).map_err(|e| format!("{}: `{key}`: {e}", self.what))
    }

    fn hash(&self, text: &str, key: &str) -> Result<DefHash, String> {
        DefHash::from_hex(text)
            .ok_or_else(|| format!("{}: `{key}` holds `{text}`, not a hash", self.what))
    }

    /// A test's or a law's `index` field, which restates the frame's number.
    fn index(&self, slot: Option<&'a str>, want: usize) -> Result<usize, String> {
        let index: usize = self.number(self.required(slot, "index")?, "index")?;
        if index != want {
            return Err(format!(
                "{}: `index` is {index}, but the frame is numbered {want}",
                self.what
            ));
        }
        Ok(index)
    }
}

struct Reader<'a> {
    sources: &'a [SourceId],
}

impl Reader<'_> {
    fn numbered(&self, name: &str, want: usize, what: &str) -> Result<(), String> {
        if name == want.to_string() {
            Ok(())
        } else {
            Err(format!(
                "{what} is numbered out of order; frame {want} was expected"
            ))
        }
    }

    /// The test or law a `testhash` / `lawhash` frame is about, which an earlier frame declared.
    fn item_index(
        &self,
        name: &str,
        declared: usize,
        of: &str,
        what: &str,
    ) -> Result<usize, String> {
        let i: usize = name
            .parse()
            .map_err(|_| format!("{what} is not numbered"))?;
        if i >= declared {
            return Err(format!(
                "{what} names {of} {i}, and only {declared} were declared"
            ));
        }
        Ok(i)
    }

    fn span(&self, text: &str, what: &str) -> Result<Span, String> {
        let words: Vec<&str> = text.split(' ').collect();
        let [module, start, end] = words[..] else {
            return Err(format!("{what}: `{text}` is not `<module> <start> <end>`"));
        };
        let number = |word: &str| {
            word.parse::<u32>()
                .map_err(|_| format!("{what}: span `{text}` holds `{word}`"))
        };
        let module = number(module)?;
        let source = if module == NO_MODULE {
            Span::DUMMY.source
        } else {
            self.sources.get(module as usize).copied().ok_or_else(|| {
                format!(
                    "{what} spans module {module}, and only {} sources were handed over",
                    self.sources.len()
                )
            })?
        };
        Ok(Span::new(source, number(start)?, number(end)?))
    }

    fn scheme(&self, text: &str, what: &str) -> Result<Scheme, String> {
        parse_scheme(text).map_err(|e| format!("{what}: scheme: {e}"))
    }

    fn ty(&self, text: &str, what: &str) -> Result<Type, String> {
        parse_type(text).map_err(|e| format!("{what}: type: {e}"))
    }

    fn def(&self, name: &str, payload: &[u8], what: &str) -> Result<DefInfo, String> {
        let f = Fields::of(payload, what)?;
        let (mut module, mut simple, mut scheme, mut footprint, mut performed) =
            (None, None, None, None, None);
        let (mut effectful, mut span) = (None, None);
        let (mut constraints, mut aliases, mut spec) = (Vec::new(), Vec::new(), Vec::new());
        for (key, text) in f.all() {
            match key {
                "module" => f.once(&mut module, key, text)?,
                "simple_name" => f.once(&mut simple, key, text)?,
                "scheme" => f.once(&mut scheme, key, text)?,
                "footprint" => f.once(&mut footprint, key, text)?,
                "performed" => f.once(&mut performed, key, text)?,
                "internally_effectful" => f.once(&mut effectful, key, text)?,
                "span" => f.once(&mut span, key, text)?,
                "constraint" => {
                    let Some((deriver, param)) = text.split_once(' ') else {
                        return Err(format!(
                            "{what}: constraint `{text}` is not `<deriver> <param>`"
                        ));
                    };
                    let deriver = Deriver::from_name(deriver)
                        .ok_or_else(|| format!("{what}: `{deriver}` is not a deriver"))?;
                    constraints.push(DefConstraint {
                        deriver,
                        param: f.number(param, key)?,
                    });
                }
                "row_alias" => aliases.push(Symbol::new(text)),
                "spec" => {
                    let Some((head, span)) = text.split_once('\n') else {
                        return Err(format!("{what}: a spec has no span line"));
                    };
                    let words: Vec<&str> = head.split(' ').collect();
                    let [kind, index, footprint] = words[..] else {
                        return Err(format!(
                            "{what}: spec `{head}` is not `<kind> <index> <footprint>`"
                        ));
                    };
                    spec.push(SpecInfo {
                        kind: spec_kind(kind, what)?,
                        index: f.number(index, key)?,
                        footprint: f.footprint(footprint, key)?,
                        span: self.span(span, what)?,
                    });
                }
                other => return Err(unknown_field(what, other)),
            }
        }
        Ok(DefInfo {
            name: Symbol::new(name),
            module: ModuleName::from_dotted(f.required(module, "module")?),
            simple_name: Symbol::new(f.required(simple, "simple_name")?),
            scheme: self.scheme(f.required(scheme, "scheme")?, what)?,
            footprint: f.footprint(f.required(footprint, "footprint")?, "footprint")?,
            performed: f.footprint(f.required(performed, "performed")?, "performed")?,
            row_aliases: aliases,
            constraints,
            spec,
            internally_effectful: f.flag(
                f.required(effectful, "internally_effectful")?,
                "internally_effectful",
            )?,
            span: self.span(f.required(span, "span")?, what)?,
        })
    }

    fn test(&self, index: usize, payload: &[u8], what: &str) -> Result<TestInfo, String> {
        let f = Fields::of(payload, what)?;
        let (mut key, mut name, mut module, mut at, mut nondet, mut footprint, mut span) =
            (None, None, None, None, None, None, None);
        for (k, text) in f.all() {
            match k {
                "key" => f.once(&mut key, k, text)?,
                "name" => f.once(&mut name, k, text)?,
                "module" => f.once(&mut module, k, text)?,
                "index" => f.once(&mut at, k, text)?,
                "nondet" => f.once(&mut nondet, k, text)?,
                "footprint" => f.once(&mut footprint, k, text)?,
                "span" => f.once(&mut span, k, text)?,
                other => return Err(unknown_field(what, other)),
            }
        }
        Ok(TestInfo {
            name: f.required(name, "name")?.to_string(),
            module: ModuleName::from_dotted(f.required(module, "module")?),
            key: Symbol::new(f.required(key, "key")?),
            index: f.index(at, index)?,
            nondet: f.flag(f.required(nondet, "nondet")?, "nondet")?,
            footprint: f.footprint(f.required(footprint, "footprint")?, "footprint")?,
            span: self.span(f.required(span, "span")?, what)?,
        })
    }

    fn law(&self, index: usize, payload: &[u8], what: &str) -> Result<LawInfo, String> {
        let f = Fields::of(payload, what)?;
        let (mut key, mut name, mut module, mut at) = (None, None, None, None);
        let (mut has_guard, mut host, mut footprint, mut span) = (None, None, None, None);
        let mut binders = Vec::new();
        for (k, text) in f.all() {
            match k {
                "key" => f.once(&mut key, k, text)?,
                "name" => f.once(&mut name, k, text)?,
                "module" => f.once(&mut module, k, text)?,
                "index" => f.once(&mut at, k, text)?,
                "has_guard" => f.once(&mut has_guard, k, text)?,
                "host" => f.once(&mut host, k, text)?,
                "footprint" => f.once(&mut footprint, k, text)?,
                "span" => f.once(&mut span, k, text)?,
                "binder" => {
                    let Some((head, ty)) = text.split_once('\n') else {
                        return Err(format!("{what}: binder `{text}` has no type line"));
                    };
                    let Some((name, span)) = head.split_once(' ') else {
                        return Err(format!("{what}: binder `{head}` is not `<name> <span>`"));
                    };
                    binders.push(LawBinder {
                        name: Symbol::new(name),
                        ty: self.ty(ty, what)?,
                        span: self.span(span, what)?,
                    });
                }
                other => return Err(unknown_field(what, other)),
            }
        }
        Ok(LawInfo {
            name: f.required(name, "name")?.to_string(),
            module: ModuleName::from_dotted(f.required(module, "module")?),
            key: Symbol::new(f.required(key, "key")?),
            index: f.index(at, index)?,
            binders,
            has_guard: f.flag(f.required(has_guard, "has_guard")?, "has_guard")?,
            host: f.flag(f.required(host, "host")?, "host")?,
            footprint: f.footprint(f.required(footprint, "footprint")?, "footprint")?,
            span: self.span(f.required(span, "span")?, what)?,
        })
    }

    fn effect(&self, name: &str, payload: &[u8], what: &str) -> Result<EffectInfo, String> {
        let f = Fields::of(payload, what)?;
        let (mut module, mut simple, mut nondet, mut span) = (None, None, None, None);
        let mut ops = IndexMap::new();
        for (key, text) in f.all() {
            match key {
                "module" => f.once(&mut module, key, text)?,
                "simple_name" => f.once(&mut simple, key, text)?,
                "nondet" => f.once(&mut nondet, key, text)?,
                "span" => f.once(&mut span, key, text)?,
                "op" => {
                    let op = self.op(&f, text, what)?;
                    if ops.insert(op.name.clone(), op).is_some() {
                        return Err(format!("{what} declares one operation twice"));
                    }
                }
                other => return Err(unknown_field(what, other)),
            }
        }
        Ok(EffectInfo {
            name: Symbol::new(name),
            module: ModuleName::from_dotted(f.required(module, "module")?),
            simple_name: Symbol::new(f.required(simple, "simple_name")?),
            nondet: f.flag(f.required(nondet, "nondet")?, "nondet")?,
            ops,
            span: self.span(f.required(span, "span")?, what)?,
        })
    }

    fn op(&self, f: &Fields<'_>, text: &str, what: &str) -> Result<OpInfo, String> {
        let Some(body) = text.strip_suffix('\n') else {
            return Err(format!("{what}: an op's last line does not end"));
        };
        let lines: Vec<&str> = body.split('\n').collect();
        let head: Vec<&str> = lines[0].split(' ').collect();
        let [
            name,
            mode,
            resource_param,
            count,
            has_scheme,
            module,
            start,
            end,
        ] = head[..]
        else {
            return Err(format!(
                "{what}: op `{}` is not `<name> <mode> <resource_param> <param count> \
                 <has_scheme> <span>`",
                lines[0]
            ));
        };
        let count: usize = f.number(count, "op")?;
        let has_scheme = f.flag(has_scheme, "op")?;
        let want = 2 + count + usize::from(has_scheme);
        if lines.len() != want {
            return Err(format!(
                "{what}: op `{name}` declares {count} parameter(s) and {} scheme but has {} \
                 line(s), not {want}",
                if has_scheme { "a" } else { "no" },
                lines.len()
            ));
        }
        let mode = match mode {
            "read" => Mode::Read,
            "write" => Mode::Write,
            other => return Err(format!("{what}: `{other}` is not `read` or `write`")),
        };
        let params = lines[1..1 + count]
            .iter()
            .map(|t| self.ty(t, what))
            .collect::<Result<Vec<_>, _>>()?;
        let scheme = if has_scheme {
            Some(self.scheme(lines[count + 2], what)?)
        } else {
            None
        };
        Ok(OpInfo {
            name: Symbol::new(name),
            mode,
            resource_param: f.flag(resource_param, "op")?,
            params,
            ret: self.ty(lines[count + 1], what)?,
            span: self.span(&format!("{module} {start} {end}"), what)?,
            scheme,
        })
    }

    fn ctor(&self, name: &str, payload: &[u8], what: &str) -> Result<CtorInfo, String> {
        let f = Fields::of(payload, what)?;
        let (mut module, mut simple, mut type_name, mut index, mut arity) =
            (None, None, None, None, None);
        let (mut scheme, mut span) = (None, None);
        let mut fields = Vec::new();
        for (key, text) in f.all() {
            match key {
                "module" => f.once(&mut module, key, text)?,
                "simple_name" => f.once(&mut simple, key, text)?,
                "type_name" => f.once(&mut type_name, key, text)?,
                "index" => f.once(&mut index, key, text)?,
                "arity" => f.once(&mut arity, key, text)?,
                "scheme" => f.once(&mut scheme, key, text)?,
                "span" => f.once(&mut span, key, text)?,
                "field" => fields.push(self.ty(text, what)?),
                other => return Err(unknown_field(what, other)),
            }
        }
        Ok(CtorInfo {
            name: Symbol::new(name),
            module: ModuleName::from_dotted(f.required(module, "module")?),
            simple_name: Symbol::new(f.required(simple, "simple_name")?),
            type_name: Symbol::new(f.required(type_name, "type_name")?),
            index: f.number(f.required(index, "index")?, "index")?,
            arity: f.number(f.required(arity, "arity")?, "arity")?,
            fields,
            scheme: self.scheme(f.required(scheme, "scheme")?, what)?,
            span: self.span(f.required(span, "span")?, what)?,
        })
    }

    fn hash(
        &self,
        name: &str,
        payload: &[u8],
        what: &str,
        out: &mut HashOutput,
    ) -> Result<(), String> {
        let f = Fields::of(payload, what)?;
        let name = Symbol::new(name);
        let (mut def, mut own, mut decl) = (None, None, None);
        let (mut specs, mut spec_texts, mut deps, mut closure) =
            (Vec::new(), Vec::new(), Vec::new(), BTreeSet::new());
        for (key, text) in f.all() {
            match key {
                "def" => f.once(&mut def, key, text)?,
                "own" => f.once(&mut own, key, text)?,
                "decl" => f.once(&mut decl, key, text)?,
                "spec" => specs.push(f.hash(text, key)?),
                "spec_text" => spec_texts.push(f.hash(text, key)?),
                "dep" => deps.push(Symbol::new(text)),
                "closure" => {
                    closure.insert(Symbol::new(text));
                }
                other => return Err(unknown_field(what, other)),
            }
        }
        if out.deps.contains_key(&name) {
            return Err(format!("{what} is written twice"));
        }
        if let Some(h) = def {
            out.defs.insert(name.clone(), f.hash(h, "def")?);
        }
        if let Some(h) = own {
            out.own.insert(name.clone(), f.hash(h, "own")?);
        }
        if let Some(h) = decl {
            out.decls.insert(name.clone(), f.hash(h, "decl")?);
        }
        if !specs.is_empty() {
            out.specs.insert(name.clone(), specs);
        }
        if !spec_texts.is_empty() {
            out.spec_texts.insert(name.clone(), spec_texts);
        }
        out.deps.insert(name.clone(), deps);
        out.closure.insert(name, closure);
        Ok(())
    }

    /// A test's or a law's hash frame: the hash, a law's text hash, and the references filed
    /// under its key — merged into what that key already holds, as the hasher merges a key two
    /// items share.
    fn item_hash(
        &self,
        payload: &[u8],
        what: &str,
        declared: &Symbol,
        law: bool,
        out: &mut HashOutput,
    ) -> Result<(DefHash, Option<DefHash>), String> {
        let f = Fields::of(payload, what)?;
        let (mut key, mut hash, mut text) = (None, None, None);
        let (mut deps, mut closure) = (Vec::new(), BTreeSet::new());
        for (k, t) in f.all() {
            match k {
                "key" => f.once(&mut key, k, t)?,
                "hash" => f.once(&mut hash, k, t)?,
                "text" if law => f.once(&mut text, k, t)?,
                "dep" => deps.push(Symbol::new(t)),
                "closure" => {
                    closure.insert(Symbol::new(t));
                }
                other => return Err(unknown_field(what, other)),
            }
        }
        let key = f.required(key, "key")?;
        if key != declared.as_str() {
            return Err(format!(
                "{what} is keyed `{key}`, but the item it numbers is `{declared}`"
            ));
        }
        let hash = f.hash(f.required(hash, "hash")?, "hash")?;
        let text = if law {
            Some(f.hash(f.required(text, "text")?, "text")?)
        } else {
            None
        };
        let known = out.deps.entry(declared.clone()).or_default();
        for d in deps {
            if !known.contains(&d) {
                known.push(d);
            }
        }
        out.closure
            .entry(declared.clone())
            .or_default()
            .extend(closure);
        Ok((hash, text))
    }
}
