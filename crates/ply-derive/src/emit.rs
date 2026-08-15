//! Generated definitions, as Ply source.
//!
//! Source text rather than a hand-built tree, for two reasons that matter more
//! than the convenience. It is reviewable — the golden pin test prints exactly
//! what a change to a deriver did, which is what makes the mandatory
//! `FRONTEND_VERSION` bump a thing a reader can check rather than remember. And
//! it is trivially deterministic: the output is a string, so "the same type
//! produces byte-identical code" is an assertion about a value rather than a
//! claim about a walk. Nothing here iterates a hash map; every list is in
//! declaration order.
//!
//! The text is parsed straight back and every span in it is replaced by the
//! `derive` item's, so no offset into this string ever reaches a diagnostic.

use crate::rules::{self, Shape};
use indexmap::IndexMap;
use ply_span::Symbol;
use ply_syntax::ast::{
    AtomExpr, Deriver, Ident, RowExpr, TypeDef, TypeDefBody, TypeExpr, VariantDef, Visibility,
};

/// The module's own parameterless type aliases, by simple name. Written types
/// are resolved through these before a `Map`'s key form is chosen, so two
/// spellings of one type cannot pick two wire formats.
pub type Aliases<'a> = IndexMap<Symbol, &'a TypeExpr>;

pub struct Emitter<'a> {
    deriver: Deriver,
    /// How a name from the deriver's runtime module is written in the module
    /// being expanded: `json::`, or empty when the deriver needs no module at
    /// all. Never bare for a deriver that has one — see
    /// [`crate::Expander::runtime_prefix`].
    runtime: String,
    /// Prefix for every binder this emitter introduces, chosen so that none of
    /// them can shadow a dictionary parameter — which is named after a type
    /// parameter, and a type parameter may be called anything.
    binder: String,
    aliases: &'a Aliases<'a>,
}

impl<'a> Emitter<'a> {
    pub fn new(
        deriver: Deriver,
        runtime: String,
        params: &[Ident],
        aliases: &'a Aliases<'a>,
    ) -> Emitter<'a> {
        let mut binder = String::from("d");
        while params
            .iter()
            .any(|p| p.name.as_str().starts_with(binder.as_str()))
        {
            binder.push('_');
        }
        Emitter {
            deriver,
            runtime,
            binder,
            aliases,
        }
    }

    /// The whole generated item, ready to parse.
    pub fn item(&self, def: &TypeDef, vis: Visibility) -> String {
        let name = rules::generated_name(self.deriver, def.name.name.as_str());
        let target = target_type(def);
        let mut out = String::new();
        if vis.is_public() {
            out.push_str("pub ");
        }
        out.push_str("fn ");
        out.push_str(&name);
        if !def.params.is_empty() {
            let ps: Vec<&str> = def.params.iter().map(|p| p.name.as_str()).collect();
            out.push('<');
            out.push_str(&ps.join(", "));
            out.push('>');
        }
        out.push('(');
        let dicts: Vec<String> = def
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name, self.dict_type(p.name.as_str())))
            .collect();
        out.push_str(&dicts.join(", "));
        out.push_str(") -> ");
        out.push_str(&self.dict_type(&target));
        if !def.params.is_empty() {
            let cs: Vec<String> = def
                .params
                .iter()
                .map(|p| format!("derivable({}, {})", self.deriver.as_str(), p.name))
                .collect();
            out.push_str(" where ");
            out.push_str(&cs.join(", "));
        }
        out.push_str(" = ");
        out.push_str(&self.dictionary(def, &target));
        out
    }

    /// `JsonCodec<a>` is nominal and ships with `std.json`; `EqDict` and
    /// `OrdDict` are written structurally instead. Ply's records are structural
    /// and an alias is transparent, so the expanded form *is* the alias — and
    /// unlike a name, it needs no import to be written, which keeps the
    /// generated definition's hash independent of what the module happened to
    /// import.
    fn dict_type(&self, arg: &str) -> String {
        match self.deriver {
            Deriver::Json => {
                format!("{}{}<{arg}>", self.runtime, self.deriver.dictionary())
            }
            Deriver::Eq => format!("{{eq: ({arg}, {arg}) -> Bool}}"),
            Deriver::Ord => format!("{{compare: ({arg}, {arg}) -> Ordering}}"),
        }
    }

    fn dictionary(&self, def: &TypeDef, target: &str) -> String {
        let b = &self.binder;
        match self.deriver {
            // Ply's `==` is structural equality over every value that is not a
            // function, and `compare_values` is the total order `Map` iterates
            // in. Walking the structure here would define a *second* order for
            // the same type, which could then disagree with the one a map uses —
            // and the `where derivable` on this signature is what makes the
            // delegation sound, because it is what refuses a dictionary
            // parameter instantiated with a function type.
            //
            // Both forms are unclaimable by the deriving module: `==` is an
            // operator, and `compare_values` is a reserved builtin. A bare
            // `compare` would not be — a module's own items shadow the prelude,
            // so `fn compare` in the deriving module would silently become the
            // order of every dictionary derived in it.
            Deriver::Eq => format!("{{eq: |{b}a: {target}, {b}b: {target}| {b}a == {b}b}}"),
            Deriver::Ord => {
                format!("{{compare: |{b}a: {target}, {b}b: {target}| compare_values({b}a, {b}b)}}")
            }
            Deriver::Json => match &def.body {
                TypeDefBody::Alias(body) => self.json_codec(body, target),
                TypeDefBody::Sum(variants) => self.json_sum(variants, target),
            },
        }
    }

    /// An expression of type `JsonCodec<te>`.
    fn json_codec(&self, te: &TypeExpr, annotation: &str) -> String {
        let rt = &self.runtime;
        match te {
            // The dictionary parameter for this type parameter, which the
            // signature bound under the parameter's own name.
            TypeExpr::Var(p) => p.name.to_string(),
            TypeExpr::Unit { .. } => format!("{rt}unit_json()"),
            TypeExpr::Record { fields, .. } => self.json_record(fields, annotation),
            TypeExpr::Con { name, args, .. } => {
                let simple = name.symbol().as_str();
                let codecs: Vec<String> = args
                    .iter()
                    .map(|a| self.json_codec(a, &render_type(a)))
                    .collect();
                // A JSON object's keys are strings, so a `Map<String, v>` has an
                // object form and nothing else does. `std.json` provides both.
                // The choice follows the key's *type* rather than its spelling:
                // an alias is transparent to the checker, so `type Key = String`
                // makes `Map<Key, Int>` and `Map<String, Int>` one type, and two
                // wire formats for one type are two codecs that substitute for
                // each other at every call site and disagree about the protocol.
                if simple == rules::MAP && args.len() == 2 && self.is_string(&args[0]) {
                    return format!("{rt}string_map_json({})", codecs[1]);
                }
                let call = format!("{}_json({})", rules::snake_case(simple), codecs.join(", "));
                match rules::shape(self.deriver, simple) {
                    // A named type is composed through by name, never inlined:
                    // `order_json`'s body then depends on `user_json`'s *hash*,
                    // which is what makes a change to `User` re-select exactly
                    // the tests that reach an `Order`.
                    Shape::Nominal => match &name.module {
                        Some(m) => format!("{}::{call}", m.name),
                        None => call,
                    },
                    _ => format!("{rt}{call}"),
                }
            }
            // Unreachable: the walk refuses a function type before anything is
            // emitted. A value rather than a panic, because a panic here would
            // take the process down while this fails to typecheck and is
            // reported as the compiler bug it would be.
            TypeExpr::Fn { .. } => String::from("()"),
        }
    }

    /// Whether a written type *is* `String`, following this module's own
    /// parameterless aliases.
    ///
    /// Only this module's, because expansion runs over one file: an alias in
    /// another module would make this file's generated definition a function of
    /// bytes gate 1 does not hash, so editing that module would leave a stale
    /// codec behind. A cross-module alias to `String` therefore still gets the
    /// pair form, and that is the price of expansion being a function of the
    /// file.
    fn is_string(&self, te: &TypeExpr) -> bool {
        let mut current = te;
        // A cyclic alias is `E0102` where it is declared; bounded so that
        // expansion answers rather than spins on the way there.
        for _ in 0..64 {
            let TypeExpr::Con { name, args, .. } = current else {
                return false;
            };
            if !name.is_bare() || !args.is_empty() {
                return false;
            }
            if name.symbol().as_str() == "String" {
                return true;
            }
            match self.aliases.get(name.symbol()) {
                Some(body) => current = body,
                None => return false,
            }
        }
        false
    }

    fn json_record(&self, fields: &[(Ident, TypeExpr)], annotation: &str) -> String {
        let (rt, b) = (&self.runtime, &self.binder);
        let entries: Vec<String> = fields
            .iter()
            .map(|(name, ty)| {
                let codec = self.json_codec(ty, &render_type(ty));
                format!(
                    "{{key: \"{name}\", value: ({codec}.encode)({b}v.{name})}}",
                    name = name.name
                )
            })
            .collect();
        let encode = format!("|{b}v: {annotation}| {rt}object([{}])", entries.join(", "));

        let built: Vec<String> = fields
            .iter()
            .enumerate()
            .map(|(i, (name, _))| format!("{}: {b}{i}", name.name))
            .collect();
        let mut decode = format!("Ok({{{}}})", built.join(", "));
        for (i, (name, ty)) in fields.iter().enumerate().rev() {
            let codec = self.json_codec(ty, &render_type(ty));
            decode = format!(
                "match {rt}field({b}j, \"{name}\", {codec}) \
                 {{Err({b}e) -> Err({b}e), Ok({b}{i}) -> {decode}}}",
                name = name.name
            );
        }
        format!("{{encode: {encode}, decode: |{b}j: {rt}Json| {decode}}}")
    }

    /// A variant is encoded by its declared name and its fields in order, so
    /// renaming a variant changes the generated body and re-selects the tests
    /// that reach it — which is correct, because the tag is what a client sees.
    fn json_sum(&self, variants: &[VariantDef], target: &str) -> String {
        let (rt, b) = (&self.runtime, &self.binder);

        let arms: Vec<String> = variants
            .iter()
            .map(|v| {
                let binders: Vec<String> = (0..v.fields.len()).map(|i| format!("{b}{i}")).collect();
                let pat = if binders.is_empty() {
                    v.name.name.to_string()
                } else {
                    format!("{}({})", v.name.name, binders.join(", "))
                };
                let values: Vec<String> = v
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, ty)| {
                        format!("({}.encode)({b}{i})", self.json_codec(ty, &render_type(ty)))
                    })
                    .collect();
                format!(
                    "{pat} -> {rt}variant(\"{}\", [{}])",
                    v.name.name,
                    values.join(", ")
                )
            })
            .collect();
        let encode = format!("|{b}v: {target}| match {b}v {{{}}}", arms.join(", "));

        let names: Vec<String> = variants
            .iter()
            .map(|v| format!("\"{}\"", v.name.name))
            .collect();
        let mut chain = format!("{rt}unknown_variant({b}t.tag, [{}])", names.join(", "));
        for v in variants.iter().rev() {
            let mut built = format!(
                "Ok({}{})",
                v.name.name,
                if v.fields.is_empty() {
                    String::new()
                } else {
                    format!(
                        "({})",
                        (0..v.fields.len())
                            .map(|i| format!("{b}{i}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            );
            for (i, ty) in v.fields.iter().enumerate().rev() {
                let codec = self.json_codec(ty, &render_type(ty));
                built = format!(
                    "match {rt}decode_and_then({rt}variant_value({b}t, {i}), {codec}.decode) \
                     {{Err({b}e) -> Err({b}e), Ok({b}{i}) -> {built}}}"
                );
            }
            chain = format!(
                "if {b}t.tag == \"{}\" {{{built}}} else {{{chain}}}",
                v.name.name
            );
        }
        let decode = format!(
            "|{b}j: {rt}Json| match {rt}variant_of({b}j) \
             {{Err({b}e) -> Err({b}e), Ok({b}t) -> {chain}}}"
        );
        format!("{{encode: {encode}, decode: {decode}}}")
    }
}

/// `Order`, or `Pair<x, y>` for a type with parameters.
pub fn target_type(def: &TypeDef) -> String {
    if def.params.is_empty() {
        return def.name.name.to_string();
    }
    let ps: Vec<&str> = def.params.iter().map(|p| p.name.as_str()).collect();
    format!("{}<{}>", def.name.name, ps.join(", "))
}

/// A type back as source, for the annotations a generated lambda needs. A
/// lambda parameter is inferred from its uses, and a field access on a type that
/// is still a variable has nothing to look the field up in.
pub fn render_type(te: &TypeExpr) -> String {
    match te {
        TypeExpr::Var(i) => i.name.to_string(),
        TypeExpr::Unit { .. } => String::from("()"),
        TypeExpr::Con { name, args, .. } => {
            if args.is_empty() {
                return name.to_string();
            }
            let args: Vec<String> = args.iter().map(render_type).collect();
            format!("{name}<{}>", args.join(", "))
        }
        TypeExpr::Record { fields, .. } => {
            let fs: Vec<String> = fields
                .iter()
                .map(|(n, t)| format!("{}: {}", n.name, render_type(t)))
                .collect();
            format!("{{{}}}", fs.join(", "))
        }
        TypeExpr::Fn {
            params,
            ret,
            effects,
            ..
        } => {
            let ps: Vec<String> = params.iter().map(render_type).collect();
            let row = effects
                .as_ref()
                .map(|r| format!(" / {}", render_row(r)))
                .unwrap_or_default();
            format!("({}) -> {}{row}", ps.join(", "), render_type(ret))
        }
    }
}

fn render_row(row: &RowExpr) -> String {
    let atoms: Vec<String> = row.atoms.iter().map(render_atom).collect();
    match &row.tail {
        Some(tail) => format!("{{{} | {}}}", atoms.join(", "), tail.name),
        None => format!("{{{}}}", atoms.join(", ")),
    }
}

fn render_atom(atom: &AtomExpr) -> String {
    let resource = atom
        .resource
        .as_ref()
        .map(|r| format!("[{}]", r.name))
        .unwrap_or_default();
    format!("{}.{}{resource}", atom.effect, atom.mode.as_str())
}
