//! `front.ply`'s `claims` read back: each `fn` body, clause and law as `code.ply` lowers it.

use ply_eval::IntTy;
use ply_eval::decode::{At, Ctor, Error};
use ply_eval::{BinOp, Lit, UnOp};
use ply_span::{SourceId, Span, Symbol};
use ply_ty::SpecKind;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const NO_MODULE: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub enum Code {
    Lit(Lit),
    Local(usize),
    Global(Symbol),
    Unary(UnOp, Box<Code>),
    Binary(BinOp, Box<Code>, Box<Code>),
    App(Box<Code>, Vec<Code>),
    If(Box<Code>, Box<Code>, Box<Code>),
    Block(Vec<Stmt>, Option<Box<Code>>),
    List(Vec<Code>),
    Record(Vec<(Symbol, Code)>),
    Field(Box<Code>, Symbol),
    Match(Box<Code>, Vec<Arm>),
    Lambda {
        params: usize,
        captures: Vec<(usize, usize)>,
        body: Box<Code>,
    },
    Region,
    Unreached,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let(usize, Code),
    LetPat(Pat, Code),
}

#[derive(Clone, Debug)]
pub enum Pat {
    Wild,
    Var(usize),
    Lit(Lit),
    Ctor(Symbol, Vec<Pat>),
    List(Vec<Pat>, Option<Box<Pat>>),
    Nested(Vec<Pat>),
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Code>,
    pub body: Code,
}

#[derive(Clone, Debug)]
pub struct Clause {
    pub span: Span,
    pub code: Code,
}

#[derive(Clone, Debug)]
pub struct Definition {
    pub params: usize,
    pub body: Code,
    pub spec: Vec<(SpecKind, Clause)>,
    pub refs: BTreeSet<Symbol>,
}

#[derive(Clone, Debug)]
pub struct Law {
    pub guard: Option<Clause>,
    pub body: Code,
}

#[derive(Clone, Debug, Default)]
pub struct Claims {
    pub defs: HashMap<Symbol, Definition>,
    pub laws: HashMap<Symbol, Law>,
    pub sums: BTreeMap<Symbol, usize>,
}

/// A `List<front.Claim>`; `sources[i]` is the source of the `i`th module the claims were asked over.
pub fn read_claims(answer: At<'_>, sources: &[SourceId]) -> Result<Claims, Error> {
    let mut claims = Claims::default();
    for claim in answer.list()? {
        let claim = claim.ctor()?;
        let x = claim.arg(0)?;
        match claim.name() {
            "ClaimDef" => {
                let mut refs = BTreeSet::new();
                let body = code(x.field("body")?, &mut refs)?;
                let spec = x.field("spec")?.items(|c| {
                    let kind = c.field("kind")?.ctor()?;
                    let kind = match kind.name() {
                        "SRequires" => SpecKind::Requires,
                        "SEnsures" => SpecKind::Ensures,
                        _ => return Err(kind.unknown()),
                    };
                    Ok((kind, clause(c.field("clause")?, sources)?))
                })?;
                let definition = Definition {
                    params: x.field("params")?.number()?,
                    body,
                    spec,
                    refs,
                };
                claims
                    .defs
                    .insert(Symbol::new(x.field("name")?.utf8()?), definition);
            }
            "ClaimLaw" => {
                let law = Law {
                    guard: match x.field("guard")?.option()? {
                        Some(guard) => Some(clause(guard, sources)?),
                        None => None,
                    },
                    body: code(x.field("body")?, &mut BTreeSet::new())?,
                };
                claims
                    .laws
                    .insert(Symbol::new(x.field("key")?.utf8()?), law);
            }
            "ClaimSum" => {
                claims.sums.insert(
                    Symbol::new(x.field("name")?.utf8()?),
                    x.field("variants")?.number()?,
                );
            }
            _ => return Err(claim.unknown()),
        }
    }
    Ok(claims)
}

/// A `front.Lowered`: a clause and where it is written.
fn clause(lowered: At<'_>, sources: &[SourceId]) -> Result<Clause, Error> {
    Ok(Clause {
        span: span(lowered.field("at")?, sources)?,
        code: code(lowered.field("code")?, &mut BTreeSet::new())?,
    })
}

fn span(at: At<'_>, sources: &[SourceId]) -> Result<Span, Error> {
    let module = at.field("module")?;
    let index: u32 = module.number()?;
    let source = if index == NO_MODULE {
        Span::DUMMY.source
    } else {
        *sources.get(index as usize).ok_or_else(|| {
            module.error(format!(
                "module {index}, and only {} sources were handed over",
                sources.len()
            ))
        })?
    };
    Ok(Span::new(
        source,
        at.field("start")?.number()?,
        at.field("end")?.number()?,
    ))
}

/// An `Option<code.Code>`: a body the lowering did not take reads as unreached. Every global it
/// names, even under an effect the prover treats as opaque, is added to `refs`.
fn code(lowered: At<'_>, refs: &mut BTreeSet<Symbol>) -> Result<Code, Error> {
    match lowered.option()? {
        Some(c) => Reader { refs }.code(c),
        None => Ok(Code::Unreached),
    }
}

struct Reader<'r> {
    refs: &'r mut BTreeSet<Symbol>,
}

impl Reader<'_> {
    fn code(&mut self, c: At<'_>) -> Result<Code, Error> {
        stacker::maybe_grow(256 * 1024, 2 * 1024 * 1024, || {
            self.node(c.field("node")?.ctor()?)
        })
    }

    fn codes(&mut self, list: At<'_>) -> Result<Vec<Code>, Error> {
        list.items(|c| self.code(c))
    }

    fn node(&mut self, node: Ctor<'_>) -> Result<Code, Error> {
        let x = node.arg(0)?;
        Ok(match node.name() {
            "NLit" => Code::Lit(lit(x)?),
            // A label argument is the field-table entry of a resource: opaque here, like a perform.
            "NLabel" => Code::Region,
            "NVar" => match x.field("slot")?.option()? {
                Some(slot) => Code::Local(slot.number()?),
                None => {
                    let name = Symbol::new(x.field("name")?.utf8()?);
                    self.refs.insert(name.clone());
                    Code::Global(name)
                }
            },
            "NUnary" => {
                let op = x.field("op")?;
                let op = match op.utf8()? {
                    "neg" => UnOp::Neg,
                    "not" => UnOp::Not,
                    "bitnot" => UnOp::BitNot,
                    other => return Err(op.error(format!("`{other}` is no unary operator"))),
                };
                Code::Unary(op, Box::new(self.code(x.field("operand")?)?))
            }
            "NBinary" => {
                let op = x.field("op")?;
                let Some(op) = binary(op.utf8()?) else {
                    return Err(op.error("no binary operator"));
                };
                let lhs = self.code(x.field("lhs")?)?;
                Code::Binary(op, Box::new(lhs), Box::new(self.code(x.field("rhs")?)?))
            }
            "NApp" => {
                let func = self.code(x.field("func")?)?;
                Code::App(Box::new(func), self.codes(x.field("args")?)?)
            }
            "NIf" => {
                let cond = self.code(x.field("cond")?)?;
                let then_branch = self.code(x.field("then_branch")?)?;
                Code::If(
                    Box::new(cond),
                    Box::new(then_branch),
                    Box::new(self.code(x.field("else_branch")?)?),
                )
            }
            "NBlock" => {
                let mut stmts = Vec::new();
                for stmt in x.field("stmts")?.list()? {
                    let stmt = stmt.ctor()?;
                    let s = stmt.arg(0)?;
                    match stmt.name() {
                        "CLet" => {
                            let value = self.code(s.field("value")?)?;
                            stmts.push(Stmt::Let(s.field("slot")?.number()?, value));
                        }
                        "CLetPat" => {
                            let pat = pat(s.field("pat")?)?;
                            let value = self.code(s.field("value")?)?;
                            stmts.push(match pat {
                                Pat::Var(slot) => Stmt::Let(slot, value),
                                pat => Stmt::LetPat(pat, value),
                            });
                        }
                        // A statement run for its effect adds nothing a claim's value is made of.
                        "CDo" => {
                            self.code(s.field("code")?)?;
                        }
                        _ => return Err(stmt.unknown()),
                    }
                }
                let tail = match x.field("tail")?.option()? {
                    Some(tail) => Some(Box::new(self.code(tail)?)),
                    None => None,
                };
                Code::Block(stmts, tail)
            }
            "NList" => Code::List(self.codes(x.field("items")?)?),
            "NRecord" => Code::Record(x.field("fields")?.items(|f| {
                Ok((
                    Symbol::new(f.field("name")?.utf8()?),
                    self.code(f.field("value")?)?,
                ))
            })?),
            "NMatch" => {
                let scrutinee = self.code(x.field("scrutinee")?)?;
                let arms = x.field("arms")?.items(|a| {
                    let pat = pat(a.field("pat")?)?;
                    let guard = match a.field("guard")?.option()? {
                        Some(guard) => Some(self.code(guard)?),
                        None => None,
                    };
                    Ok(Arm {
                        pat,
                        guard,
                        body: self.code(a.field("body")?)?,
                    })
                })?;
                Code::Match(Box::new(scrutinee), arms)
            }
            "NPerform" => {
                self.codes(x.field("args")?)?;
                Code::Region
            }
            "NCell" => {
                self.code(x.field("init")?)?;
                self.code(x.field("body")?)?;
                Code::Region
            }
            "NSim" => {
                self.code(x.field("body")?)?;
                Code::Region
            }
            "NHandle" => {
                self.code(x.field("body")?)?;
                for clause in x.field("clauses")?.list()? {
                    self.code(clause.field("body")?)?;
                }
                if let Some(ret) = x.field("ret")?.option()? {
                    self.code(ret.field("body")?)?;
                }
                Code::Region
            }
            "NLambda" => Code::Lambda {
                params: x.field("params")?.number()?,
                captures: x
                    .field("caps")?
                    .items(|cap| Ok((cap.field("src")?.number()?, cap.field("dst")?.number()?)))?,
                body: Box::new(self.code(x.field("body")?)?),
            },
            "NField" => {
                let base = self.code(x.field("base")?)?;
                Code::Field(Box::new(base), Symbol::new(x.field("field")?.utf8()?))
            }
            // The fields a record update copies read the base; the ones it sets follow them.
            "NUpd" => {
                let base = self.code(x.field("base")?)?;
                let mut fields = x.field("copies")?.items(|name| {
                    let name = Symbol::new(name.utf8()?);
                    Ok((name.clone(), Code::Field(Box::new(base.clone()), name)))
                })?;
                for set in x.field("sets")?.list()? {
                    fields.push((
                        Symbol::new(set.field("name")?.utf8()?),
                        self.code(set.field("value")?)?,
                    ));
                }
                Code::Record(fields)
            }
            _ => return Err(node.unknown()),
        })
    }
}

/// A `code.CPat`.
fn pat(p: At<'_>) -> Result<Pat, Error> {
    let p = p.ctor()?;
    if p.name() == "PxWild" {
        return Ok(Pat::Wild);
    }
    let x = p.arg(0)?;
    Ok(match p.name() {
        "PxVar" => {
            let slot = x.field("slot")?;
            match slot.option()? {
                Some(slot) => Pat::Var(slot.number()?),
                None => return Err(slot.error("a binder with no slot")),
            }
        }
        "PxLit" => Pat::Lit(lit(x)?),
        "PxCtor" => Pat::Ctor(
            Symbol::new(x.field("name")?.utf8()?),
            x.field("args")?.items(pat)?,
        ),
        "PxRecord" => Pat::Nested(x.field("fields")?.items(|f| pat(f.field("pat")?))?),
        "PxList" => Pat::List(
            x.field("items")?.items(pat)?,
            match x.field("rest")?.option()? {
                Some(rest) => Some(Box::new(pat(rest)?)),
                None => None,
            },
        ),
        _ => return Err(p.unknown()),
    })
}

/// A `patterns.Lit`.
fn lit(l: At<'_>) -> Result<Lit, Error> {
    let l = l.ctor()?;
    Ok(match l.name() {
        "LInt" => Lit::Int(l.arg(0)?.int()?),
        "LFixed" => {
            let width = l.arg(0)?;
            let name = width.utf8()?;
            let Some(ty) = IntTy::from_name(&name.to_ascii_uppercase()) else {
                return Err(width.error(format!("`{name}` is no width")));
            };
            Lit::Fixed {
                ty,
                bits: ty.normalize(l.arg(1)?.int()? as u64),
            }
        }
        "LBool" => Lit::Bool(l.arg(0)?.bool()?),
        "LStr" => Lit::Str(l.arg(0)?.utf8()?.to_string()),
        "LBytes" => Lit::Bytes(l.arg(0)?.bytes()?.to_vec()),
        // The prover reads no `Float`, so none is carried: a `Float` is never proved.
        "LFloat" => Lit::Float(f64::NAN),
        "LDec" => {
            let text = l.arg(0)?;
            let raw = text.utf8()?;
            decimal(raw).ok_or_else(|| text.error(format!("`{raw}` is no `Decimal` literal")))?
        }
        "LUnit" => Lit::Unit,
        _ => return Err(l.unknown()),
    })
}

/// `1.50m` is `(150, 2)`; a pattern's `-` is part of its source, and negates it.
fn decimal(raw: &str) -> Option<Lit> {
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    let digits: String = whole
        .chars()
        .chain(fraction.chars())
        .filter(char::is_ascii_digit)
        .collect();
    let magnitude: i128 = digits.parse().ok()?;
    Some(Lit::Decimal {
        mantissa: if raw.starts_with('-') {
            -magnitude
        } else {
            magnitude
        },
        scale: fraction.chars().filter(char::is_ascii_digit).count() as u32,
    })
}

fn binary(name: &str) -> Option<BinOp> {
    Some(match name {
        "add" => BinOp::Add,
        "sub" => BinOp::Sub,
        "mul" => BinOp::Mul,
        "div" => BinOp::Div,
        "rem" => BinOp::Rem,
        "eq" => BinOp::Eq,
        "ne" => BinOp::Ne,
        "lt" => BinOp::Lt,
        "le" => BinOp::Le,
        "gt" => BinOp::Gt,
        "ge" => BinOp::Ge,
        "and" => BinOp::And,
        "or" => BinOp::Or,
        "concat" => BinOp::Concat,
        "bitand" => BinOp::BitAnd,
        "bitor" => BinOp::BitOr,
        "bitxor" => BinOp::BitXor,
        "shl" => BinOp::Shl,
        "shr" => BinOp::Shr,
        "ushr" => BinOp::Ushr,
        _ => return None,
    })
}
