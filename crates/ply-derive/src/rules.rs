//! What a type constructor means to a derivation.

use ply_syntax::ast::Deriver;

/// How a type constructor participates in a derivation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// Encoded directly, with no structure to walk.
    Leaf,
    /// Derivable exactly when every argument is.
    Structural(usize),
    /// No total encoding exists.
    Refused(Refusal),
    /// A name the program declares.
    Nominal,
}

/// A `Map`'s key type must be ordered whatever deriver is walking it: the map's iteration order is
/// what a derived encoding of it is a function of.
pub const MAP: &str = "Map";

pub const OPTION: &str = "Option";

/// The credential type.
pub const SECRET: &str = "Secret";

/// Whether a type constructor's JSON encoding can be the document `null`.
pub fn json_null_encoded(name: &str) -> bool {
    matches!(name, "Unit" | OPTION)
}

/// Completes "…^^^^ {reason}" for an `Option` whose payload can encode as `null`.
pub fn null_in_option(inner: &str) -> String {
    format!(
        "`{inner}` encodes as `null`, which is how an `Option` writes `None`, so `Some` and \
         `None` are the same document"
    )
}

/// The advice that goes with [`null_in_option`], stated once because two crates print it.
pub const NULL_IN_OPTION_NOTE: &str = "wrap the inner value in a record or a one-field variant — both are tagged, and a tagged \
     encoding has no `null` to collide with";

/// Why a type constructor has no derivation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// `Float`, and only for `ord`.
    FloatIsNotOrdered,
    /// A `Cell` or a `Task`: a name for a location, not a value.
    Handle(&'static str),
    /// A `Secret`, for every deriver but `eq`.
    Secret(Deriver),
}

impl Refusal {
    /// Completes "…^^^^ {reason}".
    pub fn reason(self) -> String {
        match self {
            Refusal::FloatIsNotOrdered => String::from("`Float` is not ordered: `NaN != NaN`"),
            Refusal::Handle(what) => {
                format!("a `{what}` names a location rather than a value")
            }
            Refusal::Secret(deriver) => String::from(secret_reason(deriver)),
        }
    }

    /// The advice that goes with a `Secret` refusal, stated once because two crates print it.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Refusal::Secret(Deriver::Ord) => Some(
                "an ordering over a credential leaks a bit of position per comparison and \
                 recovers the value in calls proportional to its length; `derive eq` is \
                 available, and `secret_verify` is the check a program actually wants",
            ),
            Refusal::Secret(_) => Some(
                "move the `Secret` field out of the type being derived, or write the codec by \
                 hand over the fields that are not credentials — there is no way to encode a \
                 `Secret`, which is the guarantee",
            ),
            _ => None,
        }
    }
}

/// What a derivation of `deriver` would have had to do with a credential.
fn secret_reason(deriver: Deriver) -> &'static str {
    match deriver {
        Deriver::Json => "a derived codec would write the credential into the document",
        Deriver::Ord => "a credential has no order",
        // Unreachable while `eq` is the one deriver a `Secret` satisfies, and total rather than a
        // panic because `Refusal` is a plain data type.
        Deriver::Eq => "a credential has no structural equality to derive",
    }
}

pub fn shape(deriver: Deriver, name: &str) -> Shape {
    match name {
        "Int" | "Bool" | "String" | "Bytes" | "Unit" | "Decimal" => Shape::Leaf,
        // The eight fixed-width integer types are leaves for the same reason `Int` is: they have
        // structural equality, a total order, and one obvious rendering.
        "U8" | "U16" | "U32" | "U64" | "I8" | "I16" | "I32" | "I64" => Shape::Leaf,
        // A total order that disagrees with the language's `==` on its own keys is a lookup that
        // fails to find what it just inserted, and `NaN` makes `<` non-total.
        "Float" => match deriver {
            Deriver::Ord => Shape::Refused(Refusal::FloatIsNotOrdered),
            _ => Shape::Leaf,
        },
        "List" | OPTION => Shape::Structural(1),
        MAP | "Result" => Shape::Structural(2),
        // The prelude's parameterless ADTs.
        "Ordering" | "Rounding" => Shape::Structural(0),
        "Cell" => Shape::Refused(Refusal::Handle("Cell")),
        "Task" => Shape::Refused(Refusal::Handle("Task")),
        // `eq` is a `Leaf` rather than `Structural(1)`: the payload is compared by the evaluator in
        // constant time and no generated body ever names it, so requiring `derivable(eq, a)` of the
        // payload would be a constraint on a type nothing can reach.
        SECRET => match deriver {
            Deriver::Eq => Shape::Leaf,
            other => Shape::Refused(Refusal::Secret(other)),
        },
        _ => Shape::Nominal,
    }
}

/// What a function type is refused with.
pub fn function_refusal(deriver: Deriver) -> &'static str {
    match deriver {
        Deriver::Json => "a function has no JSON encoding",
        Deriver::Eq => "functions cannot be compared for equality",
        Deriver::Ord => "functions cannot be ordered",
    }
}

/// `snake_case(TypeName)`: insert `_` before an uppercase letter that follows a lowercase letter or
/// a digit, and before the last uppercase of a run that is followed by a lowercase; then lowercase
/// everything.
pub fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_numeric() || (prev.is_uppercase() && next_lower) {
                out.push('_');
            }
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// The name a derivation of `deriver` for `type_name` generates.
pub fn generated_name(deriver: Deriver, type_name: &str) -> String {
    format!("{}_{}", snake_case(type_name), deriver.as_str())
}

/// The module whose definitions a generated body calls, if any.
pub fn runtime_module(deriver: Deriver) -> Option<&'static str> {
    match deriver {
        Deriver::Json => Some("std.json"),
        Deriver::Eq | Deriver::Ord => None,
    }
}
