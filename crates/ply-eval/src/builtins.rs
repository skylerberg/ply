use crate::interp::{Interp, arity_error};
use crate::value::{Value, type_error, values_equal};
use ply_span::{Diagnostic, Span, codes};

/// A list this long is a runaway `range`, not an intent.
const MAX_RANGE_LEN: i64 = 10_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Builtin {
    Assert,
    AssertEq,
    Len,
    Push,
    Map,
    Filter,
    Fold,
    Range,
    IntToString,
    StringConcat,
    CellGet,
    CellSet,
    Panic,
}

impl Builtin {
    pub fn from_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "assert" => Builtin::Assert,
            "assert_eq" => Builtin::AssertEq,
            "len" => Builtin::Len,
            "push" => Builtin::Push,
            "map" => Builtin::Map,
            "filter" => Builtin::Filter,
            "fold" => Builtin::Fold,
            "range" => Builtin::Range,
            "int_to_string" => Builtin::IntToString,
            "string_concat" => Builtin::StringConcat,
            "cell_get" => Builtin::CellGet,
            "cell_set" => Builtin::CellSet,
            "panic" => Builtin::Panic,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Builtin::Assert => "assert",
            Builtin::AssertEq => "assert_eq",
            Builtin::Len => "len",
            Builtin::Push => "push",
            Builtin::Map => "map",
            Builtin::Filter => "filter",
            Builtin::Fold => "fold",
            Builtin::Range => "range",
            Builtin::IntToString => "int_to_string",
            Builtin::StringConcat => "string_concat",
            Builtin::CellGet => "cell_get",
            Builtin::CellSet => "cell_set",
            Builtin::Panic => "panic",
        }
    }

    /// Inclusive `(min, max)` argument counts.
    pub fn arity(self) -> (usize, usize) {
        match self {
            Builtin::Assert => (1, 2),
            Builtin::Range => (1, 2),
            Builtin::Len | Builtin::IntToString | Builtin::CellGet | Builtin::Panic => (1, 1),
            Builtin::AssertEq
            | Builtin::Push
            | Builtin::Map
            | Builtin::Filter
            | Builtin::StringConcat
            | Builtin::CellSet => (2, 2),
            Builtin::Fold => (3, 3),
        }
    }

    pub fn all() -> &'static [Builtin] {
        &[
            Builtin::Assert,
            Builtin::AssertEq,
            Builtin::Len,
            Builtin::Push,
            Builtin::Map,
            Builtin::Filter,
            Builtin::Fold,
            Builtin::Range,
            Builtin::IntToString,
            Builtin::StringConcat,
            Builtin::CellGet,
            Builtin::CellSet,
            Builtin::Panic,
        ]
    }
}

impl Interp<'_> {
    pub(crate) fn call_builtin(
        &mut self,
        b: Builtin,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let (min, max) = b.arity();
        if args.len() < min || args.len() > max {
            let expected = if min == max { min } else { max };
            return Err(arity_error(
                span,
                &format!("`{}`", b.name()),
                expected,
                args.len(),
            ));
        }

        match b {
            Builtin::Assert => {
                if args[0].as_bool(span, "`assert`")? {
                    return Ok(Value::Unit);
                }
                let mut diag = Diagnostic::error(
                    codes::ASSERTION_FAILED,
                    "assertion failed: condition is false",
                )
                .primary(span, "this condition evaluated to false");
                if let Some(message) = args.get(1) {
                    diag = diag.note(match message {
                        Value::Str(s) => s.to_string(),
                        other => other.render(),
                    });
                }
                Err(diag)
            }

            Builtin::AssertEq => {
                if values_equal(&args[0], &args[1], span)? {
                    Ok(Value::Unit)
                } else {
                    Err(self.assert_eq_failure(&args[0], &args[1], span))
                }
            }

            Builtin::Len => match &args[0] {
                Value::List(xs) => Ok(Value::Int(xs.len() as i64)),
                Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                other => Err(type_error(span, "`len`", "a List or String", other)),
            },

            Builtin::Push => {
                let xs = args[0].as_list(span, "`push`")?;
                let mut out = Vec::with_capacity(xs.len() + 1);
                out.extend(xs.iter().cloned());
                out.push(args[1].clone());
                Ok(Value::list(out))
            }

            Builtin::Map => {
                let xs = args[0].as_list(span, "`map`")?.clone();
                let f = args[1].clone();
                let mut out = Vec::with_capacity(xs.len());
                for x in xs.iter() {
                    out.push(self.apply(f.clone(), vec![x.clone()], span)?);
                }
                Ok(Value::list(out))
            }

            Builtin::Filter => {
                let xs = args[0].as_list(span, "`filter`")?.clone();
                let f = args[1].clone();
                let mut out = Vec::new();
                for x in xs.iter() {
                    let keep = self.apply(f.clone(), vec![x.clone()], span)?;
                    if keep.as_bool(span, "the predicate given to `filter`")? {
                        out.push(x.clone());
                    }
                }
                Ok(Value::list(out))
            }

            Builtin::Fold => {
                let xs = args[0].as_list(span, "`fold`")?.clone();
                let mut acc = args[1].clone();
                let f = args[2].clone();
                for x in xs.iter() {
                    acc = self.apply(f.clone(), vec![acc, x.clone()], span)?;
                }
                Ok(acc)
            }

            Builtin::Range => {
                let (lo, hi) = match args.len() {
                    1 => (0, args[0].as_int(span, "`range`")?),
                    _ => (
                        args[0].as_int(span, "`range`")?,
                        args[1].as_int(span, "`range`")?,
                    ),
                };
                if hi <= lo {
                    return Ok(Value::list(Vec::new()));
                }
                let len = hi.saturating_sub(lo);
                if len > MAX_RANGE_LEN {
                    return Err(Diagnostic::error(
                        codes::RUNTIME_ERROR,
                        format!("`range` of {len} elements exceeds the limit of {MAX_RANGE_LEN}"),
                    )
                    .primary(span, "this range is too large to materialize"));
                }
                Ok(Value::list((lo..hi).map(Value::Int).collect()))
            }

            Builtin::IntToString => Ok(Value::str(
                args[0].as_int(span, "`int_to_string`")?.to_string(),
            )),

            Builtin::StringConcat => {
                let a = args[0].as_str(span, "`string_concat`")?;
                let b = args[1].as_str(span, "`string_concat`")?;
                Ok(Value::str(format!("{a}{b}")))
            }

            Builtin::CellGet => {
                let cell = args[0].as_cell(span, "`cell_get`")?;
                let borrowed = cell.try_borrow().map_err(|_| busy_cell(span, "read"))?;
                Ok(borrowed.clone())
            }

            Builtin::CellSet => {
                let cell = args[0].as_cell(span, "`cell_set`")?.clone();
                let mut borrowed = cell
                    .try_borrow_mut()
                    .map_err(|_| busy_cell(span, "written"))?;
                *borrowed = args[1].clone();
                Ok(Value::Unit)
            }

            Builtin::Panic => {
                let message = match &args[0] {
                    Value::Str(s) => s.to_string(),
                    other => other.render(),
                };
                Err(
                    Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
                        .primary(span, "`panic` called here"),
                )
            }
        }
    }
}

fn busy_cell(span: Span, verb: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("cell cannot be {verb} while it is already borrowed"),
    )
    .primary(span, "the cell is in use by an enclosing operation")
}
