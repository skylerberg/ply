//! Human-facing rendering of types, rows and schemes.

use crate::ty::{EffectAtom, Footprint, LabelVar, Resource, Row, RowVar, Scheme, TyVar, Type};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;

/// A constructor name no lexer can produce, so a cell's region type never collides with a user's.
pub const REGION_PREFIX: &str = "#region:";

pub fn region_type_name(resource: &str) -> String {
    format!("{REGION_PREFIX}{resource}")
}

pub fn region_of(t: &Type) -> Option<&str> {
    match t {
        Type::Con(name, args) if args.is_empty() => name.as_str().strip_prefix(REGION_PREFIX),
        _ => None,
    }
}

#[derive(Default)]
pub struct Printer {
    ty_names: FxHashMap<TyVar, String>,
    row_names: FxHashMap<RowVar, String>,
    label_names: FxHashMap<LabelVar, String>,
    /// The concrete resource names this text holds: a label variable may take none of them.
    taken: FxHashSet<String>,
}

const TY_LETTERS: &[u8] = b"abcdghijklmnopqrsuvwxyz";
const ROW_LETTERS: &[u8] = b"eft";
pub(crate) const LABEL_LETTERS: &[u8] = b"lmn";

impl Printer {
    pub fn new() -> Self {
        Printer::default()
    }

    fn ty_name(&mut self, v: TyVar) -> String {
        if let Some(n) = self.ty_names.get(&v) {
            return n.clone();
        }
        let i = self.ty_names.len();
        let name = letter_name(TY_LETTERS, i);
        self.ty_names.insert(v, name.clone());
        name
    }

    fn row_name(&mut self, v: RowVar) -> String {
        if let Some(n) = self.row_names.get(&v) {
            return n.clone();
        }
        let i = self.row_names.len();
        let name = letter_name(ROW_LETTERS, i);
        self.row_names.insert(v, name.clone());
        name
    }

    /// The first label letter, then round, that no resource in this text and no other label holds,
    /// so `[l]` in a printed row or footprint means one thing.
    fn label_name(&mut self, v: LabelVar) -> String {
        if let Some(n) = self.label_names.get(&v) {
            return n.clone();
        }
        let name = (0..)
            .map(|i| letter_name(LABEL_LETTERS, i))
            .find(|n| !self.taken.contains(n) && !self.label_names.values().any(|held| held == n))
            .expect("the letters and their rounds do not run out");
        self.label_names.insert(v, name.clone());
        name
    }

    /// Every concrete resource `t` names, before a label variable in it is given a name.
    fn reserve_ty(&mut self, t: &Type) {
        match t {
            Type::Var(_) => {}
            Type::Con(_, args) => args.iter().for_each(|a| self.reserve_ty(a)),
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                params.iter().for_each(|p| self.reserve_ty(p));
                self.reserve_ty(ret);
                self.reserve_row(effects);
            }
            Type::Record(fields) => fields.values().for_each(|t| self.reserve_ty(t)),
        }
    }

    /// What a caller that lays a row out itself must hand the printer before it names anything.
    pub fn reserve_row(&mut self, r: &Row) {
        for atom in &r.atoms {
            self.reserve_atom(atom);
        }
    }

    fn reserve_atom(&mut self, a: &EffectAtom) {
        if let Resource::Named(name) = &a.resource {
            self.taken.insert(name.to_string());
        }
    }

    /// Each atom as its own string, for a caller that places them itself.
    pub fn atoms(&mut self, atoms: &BTreeSet<EffectAtom>) -> Vec<String> {
        for atom in atoms {
            self.reserve_atom(atom);
        }
        atoms.iter().map(|a| self.atom(a)).collect()
    }

    fn atom(&mut self, a: &EffectAtom) -> String {
        match &a.resource {
            Resource::Var(v) => {
                let name = self.label_name(*v);
                a.text_with(&format!("[{name}]"))
            }
            other => a.text_with(&other.to_string()),
        }
    }

    pub fn ty(&mut self, t: &Type) -> String {
        self.reserve_ty(t);
        match t {
            Type::Var(v) => self.ty_name(*v),
            Type::Con(name, args) => {
                if let Some((region, elem)) = as_cell(t) {
                    let elem = self.ty(elem);
                    return match region_of(region) {
                        Some(r) => format!("Cell[{r}]<{elem}>"),
                        None => format!("Cell<{elem}>"),
                    };
                }
                if args.is_empty() {
                    name.to_string()
                } else {
                    let args: Vec<String> = args.iter().map(|a| self.ty(a)).collect();
                    format!("{name}<{}>", args.join(", "))
                }
            }
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                let ps: Vec<String> = params.iter().map(|p| self.ty(p)).collect();
                let ret = self.ty(ret);
                let mut s = format!("({}) -> {ret}", ps.join(", "));
                if !effects.is_pure() {
                    s.push_str(" / ");
                    s.push_str(&self.row(effects));
                }
                s
            }
            Type::Record(fields) => {
                if let Some(n) = crate::ty::tuple_arity(fields.len(), |k| fields.contains_key(k)) {
                    let ts: Vec<String> = (0..n)
                        .map(|i| self.ty(&fields[&ply_span::Symbol::new(format!("_{i}"))]))
                        .collect();
                    return format!("({})", ts.join(", "));
                }
                let fs: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| {
                        let v = self.ty(v);
                        format!("{k}: {v}")
                    })
                    .collect();
                format!("{{{}}}", fs.join(", "))
            }
        }
    }

    pub fn row(&mut self, r: &Row) -> String {
        self.reserve_row(r);
        let atoms: Vec<String> = r.atoms.iter().map(|a| self.atom(a)).collect();
        match r.tail {
            None => format!("{{{}}}", atoms.join(", ")),
            Some(v) => {
                let name = self.row_name(v);
                if atoms.is_empty() {
                    name
                } else {
                    format!("{{{} | {name}}}", atoms.join(", "))
                }
            }
        }
    }

    pub fn scheme(&mut self, s: &Scheme) -> String {
        // Named in the order a call fills them, against every resource the body names, so the head
        // and the body read the same there.
        self.reserve_ty(&s.ty);
        let bound: Vec<String> = s
            .label_vars
            .iter()
            .map(|v| format!("[{}]", self.label_name(*v)))
            .collect();
        let body = self.ty(&s.ty);
        if s.ty_vars.is_empty() && s.label_vars.is_empty() && s.row_vars.is_empty() {
            return body;
        }
        let mut params: Vec<String> = s.ty_vars.iter().map(|v| self.ty_name(*v)).collect();
        params.extend(bound);
        let rows: Vec<String> = s.row_vars.iter().map(|v| self.row_name(*v)).collect();
        let head = match (params.is_empty(), rows.is_empty()) {
            (false, false) => format!("<{} | {}>", params.join(", "), rows.join(", ")),
            (false, true) => format!("<{}>", params.join(", ")),
            (true, false) => format!("<| {}>", rows.join(", ")),
            (true, true) => unreachable!(),
        };
        format!("{head}{body}")
    }

    pub fn footprint(&mut self, f: &Footprint) -> String {
        let mut bound: Vec<LabelVar> = Vec::new();
        for atom in f.atoms() {
            if let Resource::Var(v) = atom.resource
                && !bound.contains(&v)
            {
                bound.push(v);
            }
        }
        let atoms = self.atoms(&f.0).join(",");
        if bound.is_empty() {
            return atoms;
        }
        let head: Vec<String> = bound
            .iter()
            .map(|v| format!("[{}]", self.label_name(*v)))
            .collect();
        format!("<{}>{atoms}", head.join(","))
    }
}

fn as_cell(t: &Type) -> Option<(&Type, &Type)> {
    match t {
        Type::Con(name, args) if name.as_str() == "Cell" && args.len() == 2 => {
            Some((&args[0], &args[1]))
        }
        _ => None,
    }
}

pub(crate) fn letter_name(letters: &[u8], i: usize) -> String {
    let c = letters[i % letters.len()] as char;
    let round = i / letters.len();
    if round == 0 {
        c.to_string()
    } else {
        format!("{c}{round}")
    }
}

pub fn print_type(t: &Type) -> String {
    Printer::new().ty(t)
}

pub fn print_row(r: &Row) -> String {
    Printer::new().row(r)
}

pub fn print_scheme(s: &Scheme) -> String {
    Printer::new().scheme(s)
}

/// `atom,atom`, headed by the labels its atoms name as a scheme's head names its own: without the
/// binders a label a caller fills reads back as a resource of that name.
pub fn print_footprint(f: &Footprint) -> String {
    Printer::new().footprint(f)
}
