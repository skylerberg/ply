//! The syntactic half of `derivable(D, t)`.

use crate::rules::{self, Shape};
use ply_span::Span;
use ply_syntax::ast::{Deriver, TypeDef, TypeDefBody, TypeExpr};

/// Why a type cannot be derived, and where to point.
#[derive(Clone, Debug)]
pub struct Blocker {
    pub span: Span,
    /// Completes "…^^^^ {reason}".
    pub reason: String,
    /// The advice this particular refusal carries, when the reason alone does not say what to do
    /// instead.
    pub note: Option<&'static str>,
    /// Named in the note when the blocking type sits inside a variant rather than a record field.
    pub variant: Option<String>,
}

impl Blocker {
    fn new(span: Span, reason: impl Into<String>) -> Blocker {
        Blocker {
            span,
            reason: reason.into(),
            note: None,
            variant: None,
        }
    }

    fn noted(mut self, note: Option<&'static str>) -> Blocker {
        self.note = note;
        self
    }

    /// A record field is blamed as a whole — `on_complete: (Order) -> Unit` — because that is the
    /// line the user edits, whatever depth the refusal was found at.
    fn at(mut self, span: Span) -> Blocker {
        self.span = span;
        self
    }
}

/// Whether a `derive` of `deriver` for `def` can generate a total body.
pub fn check_decl(deriver: Deriver, def: &TypeDef) -> Result<(), Blocker> {
    match &def.body {
        TypeDefBody::Alias(body) => check(deriver, body),
        TypeDefBody::Sum(variants) => {
            for variant in variants {
                for field in &variant.fields {
                    check(deriver, field).map_err(|mut b| {
                        b.variant = Some(variant.name.name.to_string());
                        b
                    })?;
                }
            }
            Ok(())
        }
    }
}

/// A type parameter is derivable by assumption: the generated signature carries `where derivable(D,
/// p)`, so a call site that instantiates it with something refused is the one that fails, and it
/// fails naming its own type.
pub fn check(deriver: Deriver, te: &TypeExpr) -> Result<(), Blocker> {
    match te {
        TypeExpr::Var(_) | TypeExpr::Unit { .. } => Ok(()),
        TypeExpr::Fn { span, .. } => Err(Blocker::new(*span, rules::function_refusal(deriver))),
        TypeExpr::Record { fields, .. } => {
            for (name, ty) in fields {
                check(deriver, ty).map_err(|b| b.at(name.span.to(ty.span())))?;
            }
            Ok(())
        }
        TypeExpr::Con { name, args, span } => {
            let simple = name.symbol().as_str();
            if let Shape::Refused(refusal) = rules::shape(deriver, simple) {
                return Err(Blocker::new(
                    *span,
                    format!("{}, so `{simple}` has no derivation", refusal.reason()),
                )
                .noted(refusal.note()));
            }
            if deriver == Deriver::Json
                && simple == rules::OPTION
                && let Some(inner) = args.first()
                && let Some(rendered) = json_null_encoded(inner)
            {
                return Err(Blocker::new(*span, rules::null_in_option(&rendered)));
            }
            // A `Map`'s iteration order is what any encoding of it is a function of, so its key
            // type must be ordered whichever deriver is walking.
            if simple == rules::MAP
                && let Some(key) = args.first()
            {
                check(Deriver::Ord, key).map_err(|b| Blocker {
                    reason: format!("{}, and a `Map` key type must be ordered", b.reason),
                    ..b
                })?;
            }
            for arg in args {
                check(deriver, arg)?;
            }
            Ok(())
        }
    }
}

/// The written type, when its JSON encoding is `null` for some value.
fn json_null_encoded(te: &TypeExpr) -> Option<String> {
    match te {
        TypeExpr::Unit { .. } => Some(String::from("Unit")),
        TypeExpr::Con { name, .. } if rules::json_null_encoded(name.symbol().as_str()) => {
            Some(name.symbol().to_string())
        }
        _ => None,
    }
}
