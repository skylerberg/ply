//! A run's configuration as the program hands it to a binding, resolved before anything is bound,
//! and the one step of resolving it that needs the target's own unit: entering the schema.

use crate::payload::{field_of, option_of};
use ply_eval::{CheckOutput, Diagnostic, Span, Symbol, Value as PlyValue, codes};
use ply_host::config::{Entry, Snapshot};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What a binding answers from and discloses: the snapshot, the named schema's keys, and whether
/// any source was read.
#[derive(Clone, Default)]
pub struct Configuration {
    /// Shared with the `Host`; immutable, so host-backed tests reading it are not coupled.
    pub snapshot: Arc<Snapshot>,
    pub schema: Option<SchemaView>,
    opened: bool,
}

/// The `--config-schema` function's name and each key's name and shape, never values: the digest
/// covers this.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SchemaView {
    pub name: String,
    pub keys: Vec<(String, String)>,
}

impl Configuration {
    /// The `Configured` record the program resolved.
    pub fn of(value: &PlyValue, span: Span) -> Result<Configuration, Diagnostic> {
        let mut values = BTreeMap::new();
        for item in field_of(value, "values", span)?.as_list(span, "the resolved values")? {
            values.insert(
                field_of(item, "key", span)?
                    .as_str(span, "a key")?
                    .to_string(),
                Entry {
                    value: field_of(item, "value", span)?
                        .as_str(span, "a value")?
                        .to_string(),
                    secret: field_of(item, "secret", span)?
                        .as_bool(span, "whether it is secret")?,
                },
            );
        }
        let schema = match option_of(field_of(value, "schema", span)?, "schema", span)? {
            None => None,
            Some(view) => {
                let mut keys = Vec::new();
                for key in field_of(view, "keys", span)?.as_list(span, "the schema's keys")? {
                    keys.push((
                        field_of(key, "name", span)?
                            .as_str(span, "a key's name")?
                            .to_string(),
                        field_of(key, "shape", span)?
                            .as_str(span, "a key's shape")?
                            .to_string(),
                    ));
                }
                Some(SchemaView {
                    name: field_of(view, "function", span)?
                        .as_str(span, "the schema function")?
                        .to_string(),
                    keys,
                })
            }
        };
        Ok(Configuration {
            snapshot: Arc::new(Snapshot::new(values, schema.is_some())),
            schema,
            opened: field_of(value, "opened", span)?.as_bool(span, "whether a source was read")?,
        })
    }

    pub fn is_opened(&self) -> bool {
        self.opened
    }

    /// Only a named schema is pinned, so the digest does not depend on whether `--host` was passed.
    pub fn is_pinned(&self) -> bool {
        self.schema.is_some()
    }

    /// The schema function's name and every key's name and shape; never values or sources.
    pub fn digest_into(&self, write: &mut dyn FnMut(&str)) {
        let Some(view) = &self.schema else {
            return;
        };
        write(&view.name);
        for (name, shape) in &view.keys {
            write(name);
            write(shape);
        }
    }
}

/// The value of the nullary definition `--config-schema` names, entered on `provider`, as data that
/// crosses threads. Whether it is a `ConfigSpec` is the program's to read.
pub fn schema_of(
    check: &CheckOutput,
    provider: Option<&'static dyn ply_eval::Provider>,
    name: &str,
) -> Result<ply_eval::Plain, Diagnostic> {
    let Some(def) = check.defs.get(&Symbol::new(name)) else {
        return Err(absent(name));
    };
    crate::support::enter_constant(provider, name)
        .map(|value| ply_eval::Plain::of(&value))
        .map_err(|failure| {
            Diagnostic::error(
                codes::CONFIG_UNAVAILABLE,
                format!("`--config-schema {name}` could not be evaluated: {}", failure.message),
            )
            .primary(def.span, "this function decides what the run requires of its configuration")
            .note("it is called once, before anything is bound, and a run that cannot compute its schema does not know whether it is configured")
        })
}

/// What a `schema` operation answers: `Result<Value, List<Diag>>`.
pub fn schema_answer(evaluated: Result<ply_eval::Plain, Diagnostic>) -> PlyValue {
    match evaluated {
        Ok(plain) => PlyValue::ctor("Ok", vec![ply_eval::reflect::value_of(&plain)]),
        Err(diagnostic) => PlyValue::ctor("Err", vec![crate::payload::diags_value(&[diagnostic])]),
    }
}

/// The CLI resolves a program's own before it configures a machine, so what this refuses is an
/// artifact's run, and an artifact carries the function only when its build shipped it.
fn absent(name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::CONFIG_UNAVAILABLE,
        format!("`--config-schema {name}` names no definition in this program"),
    )
    .primary(Span::DUMMY, "this argument is the run's configuration")
    .note("an artifact carries a schema only when `ply build --config-schema` shipped it")
}
