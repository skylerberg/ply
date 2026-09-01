//! Human-facing rendering of types, rows and schemes.

use crate::ty::{Row, RowVar, Scheme, TyVar, Type};
use rustc_hash::FxHashMap;

/// A cell's region has to be visible to inference — it decides which resource the `cell.read` /
/// `cell.write` atoms name — but must not collide with a user type, hence a constructor name no
/// lexer can produce.
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
}

const TY_LETTERS: &[u8] = b"abcdghijklmnopqrsuvwxyz";
const ROW_LETTERS: &[u8] = b"eft";

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

    pub fn ty(&mut self, t: &Type) -> String {
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
        let atoms: Vec<String> = r.atoms.iter().map(|a| a.to_string()).collect();
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
        let body = self.ty(&s.ty);
        if s.ty_vars.is_empty() && s.row_vars.is_empty() {
            return body;
        }
        let tys: Vec<String> = s.ty_vars.iter().map(|v| self.ty_name(*v)).collect();
        let rows: Vec<String> = s.row_vars.iter().map(|v| self.row_name(*v)).collect();
        let head = match (tys.is_empty(), rows.is_empty()) {
            (false, false) => format!("<{} | {}>", tys.join(", "), rows.join(", ")),
            (false, true) => format!("<{}>", tys.join(", ")),
            (true, false) => format!("<| {}>", rows.join(", ")),
            (true, true) => unreachable!(),
        };
        format!("{head}{body}")
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

fn letter_name(letters: &[u8], i: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::{EffectAtom, Resource, RowVar, TyVar};
    use ply_span::Symbol;
    use ply_syntax::ast::Mode;

    #[test]
    fn variables_are_renamed_per_item_starting_at_a() {
        let t = Type::Fn {
            params: vec![Type::Var(TyVar(7))],
            ret: Box::new(Type::Var(TyVar(7))),
            effects: Row::empty(),
        };
        assert_eq!(print_type(&t), "(a) -> a");
    }

    #[test]
    fn a_function_parameter_that_is_itself_a_function_keeps_its_own_argument_list() {
        let inner = Type::Fn {
            params: vec![Type::int()],
            ret: Box::new(Type::int()),
            effects: Row::empty(),
        };
        let outer = Type::Fn {
            params: vec![inner],
            ret: Box::new(Type::int()),
            effects: Row::empty(),
        };
        assert_eq!(print_type(&outer), "((Int) -> Int) -> Int");
    }

    #[test]
    fn a_bare_row_variable_prints_without_braces() {
        let t = Type::Fn {
            params: vec![],
            ret: Box::new(Type::unit()),
            effects: Row::open(RowVar(3)),
        };
        assert_eq!(print_type(&t), "() -> Unit / e");
    }

    #[test]
    fn atoms_and_a_tail_print_together() {
        let atom = EffectAtom::new("db", Resource::Named(Symbol::new("users")), Mode::Read);
        let row = Row {
            atoms: [atom].into(),
            tail: Some(RowVar(0)),
        };
        assert_eq!(print_row(&row), "{db.read[users] | e}");
    }

    #[test]
    fn a_cell_hides_its_phantom_region_but_names_the_resource() {
        let cell = Type::Con(
            Symbol::new("Cell"),
            vec![Type::con(&region_type_name("users")), Type::int()],
        );
        assert_eq!(print_type(&cell), "Cell[users]<Int>");
        let unknown = Type::Con(Symbol::new("Cell"), vec![Type::Var(TyVar(0)), Type::int()]);
        assert_eq!(print_type(&unknown), "Cell<Int>");
    }

    #[test]
    fn a_scheme_prints_its_quantifiers() {
        let s = Scheme {
            ty_vars: vec![TyVar(0), TyVar(1)],
            row_vars: vec![RowVar(0)],
            ty: Type::Fn {
                params: vec![Type::Var(TyVar(0))],
                ret: Box::new(Type::Var(TyVar(1))),
                effects: Row::open(RowVar(0)),
            },
        };
        assert_eq!(print_scheme(&s), "<a, b | e>(a) -> b / e");
    }
}
