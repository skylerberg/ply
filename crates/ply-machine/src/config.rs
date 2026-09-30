//! How a run is told what its configuration is, and what it refuses before it starts.

use ply_host::config::{Key, Shape, Snapshot, Sources, Spec};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::CheckOutput;
use serde_json::{Value as Json, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Configuration sources, as plain data the machines read; the shell's flags convert into this.
#[derive(Clone, Debug, Default)]
pub struct ConfigOptions {
    /// A configuration value: `--set DESK_REGION=eu`. Repeatable; highest precedence, last wins.
    pub set: Vec<String>,
    /// A `KEY=VALUE` file, one pair per line, no quoting. Repeatable; a later file wins.
    pub files: Vec<PathBuf>,
    /// `<module>.<fn>`: a nullary pure function returning a `ConfigSpec`, resolved by the program
    /// that configures the machine, or by the build that shipped it in an artifact.
    pub schema: Option<String>,
}

impl ConfigOptions {
    /// The sources, read once before anything is bound; `None` without `--host`.
    pub fn read(&self, host: bool) -> Result<Option<Sources>, Vec<Diagnostic>> {
        if !host {
            return Ok(None);
        }
        Sources::read(&self.set, &self.files).map(Some)
    }
}

/// A run's resolved configuration: the snapshot the handlers answer from.
#[derive(Clone, Default)]
pub struct Configuration {
    /// Shared with the `Host`; immutable, so host-backed tests reading it are not coupled.
    pub snapshot: Arc<Snapshot>,
    /// The `--config-schema` function's name and declared keys.
    pub schema: Option<SchemaView>,
}

/// Key names and shapes from the named schema, never values: the digest covers this.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SchemaView {
    pub name: String,
    pub keys: Vec<(String, Shape)>,
}

impl Configuration {
    /// Read sources, materialise the schema, check every key; warnings are returned, not printed.
    /// `constant` enters a nullary definition on the run's compiled unit.
    pub fn open(
        check: &CheckOutput,
        host: bool,
        options: &ConfigOptions,
        constant: &dyn Fn(&str) -> Result<ply_eval::Value, Diagnostic>,
    ) -> Result<(Configuration, Vec<Diagnostic>), Vec<Diagnostic>> {
        let Some(sources) = options.read(host)? else {
            return Ok((Configuration::default(), Vec::new()));
        };
        let spec = match &options.schema {
            None => None,
            Some(name) => Some(schema::materialise(check, name, constant).map_err(|d| vec![d])?),
        };
        let report = Snapshot::resolve(&sources, spec.as_ref())?;
        let view = options.schema.as_ref().map(|name| SchemaView {
            name: name.clone(),
            keys: spec
                .iter()
                .flat_map(|s| s.keys.iter())
                .map(|k| (k.name.clone(), k.shape))
                .collect(),
        });
        Ok((
            Configuration {
                snapshot: Arc::new(report.snapshot),
                schema: view,
            },
            report.warnings,
        ))
    }

    pub fn is_opened(&self) -> bool {
        self.snapshot.has_spec()
            || self.snapshot.environment > 0
            || self.snapshot.sets > 0
            || !self.snapshot.files.is_empty()
    }

    /// The one line the start-up banner carries.
    pub fn banner(&self) -> String {
        let counts = self.snapshot.counts();
        let mut parts = vec![format!(
            "{} {}",
            counts.keys,
            crate::support::plural(counts.keys, "key")
        )];
        for (n, what) in [
            (counts.environment, "environment"),
            (counts.set, "--set"),
            (counts.file, "--config"),
            (counts.default, "default"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        }
        if counts.secret > 0 {
            parts.push(format!("{} secrets (values not shown)", counts.secret));
        }
        parts.join(" · ")
    }

    /// The `--json` object: keys and their sources, never a secret's value.
    pub fn to_json(&self) -> Json {
        let counts = self.snapshot.counts();
        json!({
            "sources": {
                "set": self.snapshot.sets,
                "files": self.snapshot.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "environment": self.snapshot.environment,
            },
            "schema": self.schema.as_ref().map(|view| json!({
                "function": view.name,
                "keys": view.keys.iter().map(|(name, shape)| json!({
                    "name": name,
                    "shape": shape.as_str(),
                })).collect::<Vec<_>>(),
            })),
            "resolved": counts.keys,
            "secret": counts.secret,
            "keys": self.snapshot.declared().map(|(name, resolved)| json!({
                "name": name,
                "value": resolved.shown(),
                "source": resolved.source.as_str(),
                "secret": resolved.shape == Some(Shape::Secret),
            })).collect::<Vec<_>>(),
        })
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
            write(shape.as_str());
        }
    }
}

/// Reading the `--config-schema` function's value. Which definition the flag names, and whether it
/// is a nullary pure function returning a `ConfigSpec`, is decided before the machine is
/// configured: by the CLI over the program it loaded, or by the build that shipped it in an
/// artifact.
pub mod schema {
    use super::*;

    const KEYS: &str = "keys";

    /// Evaluate and decode; a definition that will not evaluate is still a refusal.
    pub fn materialise(
        check: &CheckOutput,
        name: &str,
        constant: &dyn Fn(&str) -> Result<ply_eval::Value, Diagnostic>,
    ) -> Result<Spec, Diagnostic> {
        let Some(def) = check.defs.get(&Symbol::new(name)) else {
            return Err(absent(name));
        };
        let value = constant(name).map_err(|failure| {
            Diagnostic::error(
                codes::CONFIG_UNAVAILABLE,
                format!("`--config-schema {name}` could not be evaluated: {}", failure.message),
            )
            .primary(def.span, "this function decides what the run requires of its configuration")
            .note("it is called once, before anything is bound, and a run that cannot compute its schema does not know whether it is configured")
        })?;
        spec_of(&value, name)
    }

    /// `{ keys: List<Key> }`, checked on the value: a partial decode would drop a required key.
    pub fn spec_of(value: &ply_eval::Value, name: &str) -> Result<Spec, Diagnostic> {
        use ply_eval::Value;
        let Value::Record(fields) = value else {
            return Err(malformed(name, "it is not a record"));
        };
        let Some(Value::List(keys)) = fields.get(&Symbol::new(KEYS)) else {
            return Err(malformed(name, "it has no `keys` list"));
        };
        let mut out = Vec::with_capacity(keys.len());
        for key in keys.iter() {
            out.push(key_of(key, name)?);
        }
        Spec::new(out)
    }

    fn key_of(value: &ply_eval::Value, name: &str) -> Result<Key, Diagnostic> {
        use ply_eval::Value;
        let Value::Record(fields) = value else {
            return Err(malformed(name, "a key in `keys` is not a record"));
        };
        let field = |field: &str| fields.get(&Symbol::new(field));
        let Some(Value::Str(key)) = field("name") else {
            return Err(malformed(name, "a key has no `name` string"));
        };
        let Some(Value::Ctor { name: shape, .. }) = field("shape") else {
            return Err(malformed(name, &format!("`{key}` has no `shape`")));
        };
        // Qualified, so a `SText` that some other module declared is not read as `std.config`'s.
        let Some(shape) = shape
            .as_str()
            .strip_prefix(ply_host::config::MODULE)
            .and_then(|rest| rest.strip_prefix('.'))
            .and_then(Shape::from_ctor)
        else {
            return Err(malformed(
                name,
                &format!("`{key}` has the shape `{shape}`, which is not one of `std.config`'s"),
            ));
        };
        let Some(Value::Bool(required)) = field("required") else {
            return Err(malformed(name, &format!("`{key}` has no `required` flag")));
        };
        let default = match field("default") {
            Some(Value::Ctor { name: ctor, args }) if ctor.as_str() == "Some" => {
                match args.first() {
                    Some(Value::Str(text)) => Some(text.to_string()),
                    _ => {
                        return Err(malformed(
                            name,
                            &format!("`{key}`'s default is not a string"),
                        ));
                    }
                }
            }
            Some(Value::Ctor { name: ctor, .. }) if ctor.as_str() == "None" => None,
            _ => return Err(malformed(name, &format!("`{key}` has no `default`"))),
        };
        Ok(Key {
            name: key.to_string(),
            shape,
            required: *required,
            default,
        })
    }

    fn malformed(name: &str, why: &str) -> Diagnostic {
        Diagnostic::error(
            codes::CONFIG_UNAVAILABLE,
            format!("`--config-schema {name}` returned something that is not a `ConfigSpec`: {why}"),
        )
        .primary(Span::DUMMY, "this value decides what the run requires of its configuration")
        .note("a `ConfigSpec` is `{keys: List<Key>}`, and a `Key` is `{name, shape, required, default}`")
        .note("build it with `std.config`'s `spec`, `required`, `optional` and `with_default`")
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
}
