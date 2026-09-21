//! Reads back the text [`crate::print`] writes, so `print(parse(print(t))) == print(t)`; the
//! printer is not injective (`Cell<Int>` hides its region variable).

use crate::ty::{
    EffectAtom, Footprint, LabelVar, Mode, Resource, Row, RowVar, Scheme, TyVar, Type,
};
use ply_span::Symbol;
use std::collections::BTreeMap;

const TY_LETTERS: &str = "abcdghijklmnopqrsuvwxyz";
const ROW_LETTERS: &str = "eft";
const LABEL_LETTERS: &str = "lmn";

pub fn parse_type(text: &str) -> Result<Type, String> {
    let mut p = Parser::new(text);
    let t = p.ty()?;
    p.end()?;
    Ok(t)
}

pub fn parse_scheme(text: &str) -> Result<Scheme, String> {
    let mut p = Parser::new(text);
    let s = p.scheme()?;
    p.end()?;
    Ok(s)
}

pub fn parse_row(text: &str) -> Result<Row, String> {
    let mut p = Parser::new(text);
    let r = p.row()?;
    p.end()?;
    Ok(r)
}

pub fn parse_atom(text: &str) -> Result<EffectAtom, String> {
    let mut p = Parser::new(text);
    let a = p.atom()?;
    p.end()?;
    Ok(a)
}

/// `atom,atom`, with no spaces, optionally headed by the labels its atoms bind (`<[l]>a,b`), or
/// nothing at all for the empty footprint.
pub fn parse_footprint(text: &str) -> Result<Footprint, String> {
    if text.is_empty() {
        return Ok(Footprint::empty());
    }
    let mut p = Parser::new(text);
    let f = p.footprint()?;
    p.end()?;
    Ok(f)
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
    ty_vars: Vec<(String, TyVar)>,
    row_vars: Vec<(String, RowVar)>,
    /// Bound by the scheme's head; a resource named anything else is a label in its own right.
    label_vars: Vec<(String, LabelVar)>,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Parser {
            text,
            at: 0,
            ty_vars: Vec::new(),
            row_vars: Vec::new(),
            label_vars: Vec::new(),
        }
    }

    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(' ') {
            self.at += 1;
        }
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip_spaces();
        if self.rest().starts_with(token) {
            self.at += token.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<(), String> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(self.error(&format!("expected `{token}`")))
        }
    }

    fn end(&mut self) -> Result<(), String> {
        self.skip_spaces();
        if self.at == self.text.len() {
            Ok(())
        } else {
            Err(self.error("expected the end of the text"))
        }
    }

    fn error(&self, what: &str) -> String {
        format!("{what} at byte {} of `{}`", self.at, self.text)
    }

    /// Identifier characters, the dots of a program-wide name, and `#`/`:` in a region name.
    fn name(&mut self, what: &str) -> Result<&'a str, String> {
        self.skip_spaces();
        let start = self.at;
        let region = self.peek() == Some('#');
        let in_name =
            |c: char| crate::is_ident_continue(c) || c == '.' || (region && (c == ':' || c == '#'));
        let len = self
            .rest()
            .char_indices()
            .find(|(_, c)| !in_name(*c))
            .map_or(self.rest().len(), |(i, _)| i);
        if len == 0 {
            return Err(self.error(&format!("expected {what}")));
        }
        self.at += len;
        Ok(&self.text[start..start + len])
    }

    fn ty_var(&mut self, name: &str) -> TyVar {
        if let Some((_, v)) = self.ty_vars.iter().find(|(n, _)| n == name) {
            return *v;
        }
        let v = TyVar(self.ty_vars.len() as u32);
        self.ty_vars.push((name.to_string(), v));
        v
    }

    fn row_var(&mut self, name: &str) -> RowVar {
        if let Some((_, v)) = self.row_vars.iter().find(|(n, _)| n == name) {
            return *v;
        }
        let v = RowVar(self.row_vars.len() as u32);
        self.row_vars.push((name.to_string(), v));
        v
    }

    fn label_var(&mut self, name: &str) -> LabelVar {
        if let Some((_, v)) = self.label_vars.iter().find(|(n, _)| n == name) {
            return *v;
        }
        let v = LabelVar(self.label_vars.len() as u32);
        self.label_vars.push((name.to_string(), v));
        v
    }

    /// A variable the region of a `Cell<T>` was printed without.
    fn hidden_ty_var(&mut self) -> TyVar {
        let v = TyVar(self.ty_vars.len() as u32);
        self.ty_vars.push((String::new(), v));
        v
    }

    /// The binders come first, so an atom's `[l]` is read against them as it is inside a scheme.
    fn footprint(&mut self) -> Result<Footprint, String> {
        if self.eat("<") {
            loop {
                self.expect("[")?;
                let name = self.name("a label variable")?;
                if !is_var(name, LABEL_LETTERS) {
                    return Err(self.error(&format!("`{name}` is not a label variable")));
                }
                self.label_var(name);
                self.expect("]")?;
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(">")?;
        }
        let mut atoms = Vec::new();
        loop {
            atoms.push(self.atom()?);
            if !self.eat(",") {
                break;
            }
        }
        Ok(Footprint::from_atoms(atoms))
    }

    fn scheme(&mut self) -> Result<Scheme, String> {
        let (mut ty_vars, mut row_vars, mut label_vars) = (Vec::new(), Vec::new(), Vec::new());
        if self.eat("<") {
            if !self.eat("|") {
                loop {
                    if self.eat("[") {
                        let name = self.name("a label variable")?;
                        if !is_var(name, LABEL_LETTERS) {
                            return Err(self.error(&format!("`{name}` is not a label variable")));
                        }
                        label_vars.push(self.label_var(name));
                        self.expect("]")?;
                    } else {
                        let name = self.name("a type variable")?;
                        if !is_var(name, TY_LETTERS) {
                            return Err(self.error(&format!("`{name}` is not a type variable")));
                        }
                        ty_vars.push(self.ty_var(name));
                    }
                    if !self.eat(",") {
                        break;
                    }
                }
                if !self.eat("|") {
                    self.expect(">")?;
                    let ty = self.ty()?;
                    return Ok(Scheme {
                        ty_vars,
                        row_vars,
                        label_vars,
                        ty,
                    });
                }
            }
            loop {
                let name = self.name("a row variable")?;
                if !is_var(name, ROW_LETTERS) {
                    return Err(self.error(&format!("`{name}` is not a row variable")));
                }
                row_vars.push(self.row_var(name));
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(">")?;
        }
        let ty = self.ty()?;
        Ok(Scheme {
            ty_vars,
            row_vars,
            label_vars,
            ty,
        })
    }

    fn ty(&mut self) -> Result<Type, String> {
        self.skip_spaces();
        match self.peek() {
            Some('(') => self.parens(),
            Some('{') => self.record(),
            _ => {
                let name = self.name("a type")?;
                if is_var(name, TY_LETTERS) {
                    return Ok(Type::Var(self.ty_var(name)));
                }
                self.con(name)
            }
        }
    }

    fn con(&mut self, name: &str) -> Result<Type, String> {
        if name == "Cell" {
            if self.eat("[") {
                let region = self.name("a region")?;
                self.expect("]")?;
                self.expect("<")?;
                let elem = self.ty()?;
                self.expect(">")?;
                return Ok(Type::Con(
                    Symbol::new(name),
                    vec![Type::con(&crate::print::region_type_name(region)), elem],
                ));
            }
            if self.eat("<") {
                let elem = self.ty()?;
                self.expect(">")?;
                let region = Type::Var(self.hidden_ty_var());
                return Ok(Type::Con(Symbol::new(name), vec![region, elem]));
            }
            return Ok(Type::con(name));
        }
        let mut args = Vec::new();
        if self.eat("<") {
            loop {
                args.push(self.ty()?);
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(">")?;
        }
        Ok(Type::Con(Symbol::new(name), args))
    }

    /// A parameter list or a tuple, decided by whether `->` follows.
    fn parens(&mut self) -> Result<Type, String> {
        self.expect("(")?;
        let mut items = Vec::new();
        if !self.eat(")") {
            loop {
                items.push(self.ty()?);
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(")")?;
        }
        if self.eat("->") {
            let ret = Box::new(self.ty()?);
            let effects = if self.eat("/") {
                self.row()?
            } else {
                Row::empty()
            };
            return Ok(Type::Fn {
                params: items,
                ret,
                effects,
            });
        }
        if items.len() < 2 {
            return Err(self.error("expected `->` after a parameter list, or a second tuple item"));
        }
        let fields = items
            .into_iter()
            .enumerate()
            .map(|(i, t)| (Symbol::new(format!("_{i}")), t))
            .collect();
        Ok(Type::Record(fields))
    }

    fn record(&mut self) -> Result<Type, String> {
        self.expect("{")?;
        let mut fields = BTreeMap::new();
        if self.eat("}") {
            return Ok(Type::Record(fields));
        }
        loop {
            let field = self.name("a field name")?;
            self.expect(":")?;
            let ty = self.ty()?;
            if fields.insert(Symbol::new(field), ty).is_some() {
                return Err(self.error(&format!("field `{field}` is written twice")));
            }
            if !self.eat(",") {
                break;
            }
        }
        self.expect("}")?;
        Ok(Type::Record(fields))
    }

    fn row(&mut self) -> Result<Row, String> {
        if !self.eat("{") {
            let name = self.name("a row")?;
            if !is_var(name, ROW_LETTERS) {
                return Err(self.error(&format!("`{name}` is not a row variable")));
            }
            return Ok(Row::open(self.row_var(name)));
        }
        let mut row = Row::empty();
        if self.eat("}") {
            return Ok(row);
        }
        if !self.at_char('|') {
            loop {
                row.atoms.insert(self.atom()?);
                if !self.eat(",") {
                    break;
                }
            }
        }
        if self.eat("|") {
            let name = self.name("a row variable")?;
            if !is_var(name, ROW_LETTERS) {
                return Err(self.error(&format!("`{name}` is not a row variable")));
            }
            row.tail = Some(self.row_var(name));
        }
        self.expect("}")?;
        Ok(row)
    }

    fn at_char(&mut self, c: char) -> bool {
        self.skip_spaces();
        self.peek() == Some(c)
    }

    fn atom(&mut self) -> Result<EffectAtom, String> {
        let name = self.name("an effect atom")?;
        let Some((effect, access)) = name.rsplit_once('.') else {
            return Err(self.error(&format!("`{name}` has no `.read`, `.write` or `.op`")));
        };
        if effect.is_empty() || access.is_empty() {
            return Err(self.error(&format!("`{name}` is not an effect atom")));
        }
        let resource = if self.rest().starts_with('[') {
            self.at += 1;
            let resource = self.name("a resource")?;
            self.expect("]")?;
            match self.label_vars.iter().find(|(n, _)| n == resource) {
                Some((_, v)) => Resource::Var(*v),
                None => Resource::Named(Symbol::new(resource)),
            }
        } else {
            Resource::Singleton
        };
        Ok(match access {
            "read" => EffectAtom::new(effect, resource, Mode::Read),
            "write" => EffectAtom::new(effect, resource, Mode::Write),
            // The declaration decides an operation's mode; `read_front` resolves it.
            op => EffectAtom::operation(effect, resource, Mode::Write, op),
        })
    }
}

/// Whether `name` is one the printer gives a variable: one of `letters`, then the round.
fn is_var(name: &str, letters: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| letters.contains(c)) && chars.all(|c| c.is_ascii_digit())
}
