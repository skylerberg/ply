//! How a run is told what its configuration is, and what it refuses before it starts.

use clap::Args;
use ply_core::CheckOutput;
use ply_core::ty::Type;
use ply_host::config::{Key, Shape, Snapshot, Sources, Spec};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Program;
use ply_syntax::resolve::Resolved;
use serde_json::{Value as Json, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Where a run's configuration comes from, on the command line.
///
/// The process environment is the third source and has no flag, because it has
/// no name to give: a key is one string in one namespace across all four
/// sources, so "which key was that" has one answer. There is no `PLY_` prefix,
/// no upper-casing and no `.`-to-`_` translation.
#[derive(Args, Clone, Debug, Default)]
pub struct ConfigOptions {
    /// A configuration value: `--set DESK_REGION=eu`. Repeatable, highest
    /// precedence, and the last one for a key wins.
    #[arg(
        id = "config_set",
        long = "set",
        value_name = "KEY=VALUE",
        requires = "host"
    )]
    pub set: Vec<String>,

    /// A `KEY=VALUE` file, one pair per line. Repeatable, and a later file wins
    /// over an earlier one. No quoting, no interpolation, no sections: the
    /// effect returns `Option<String>`, and a format richer than the type it
    /// feeds is a format whose extra structure is silently dropped.
    #[arg(
        id = "config_files",
        long = "config",
        value_name = "PATH",
        requires = "host"
    )]
    pub files: Vec<PathBuf>,

    /// `<module>.<fn>` — a nullary pure function returning a `ConfigSpec`,
    /// resolved against the sources at start-up so a missing or malformed value
    /// is `E0441`/`E0442` before anything is bound rather than a `None` two
    /// hundred requests in.
    ///
    /// It is also what decides which keys are credentials: a key declared
    /// `SSecret` is readable only through `config.secret`, which answers a
    /// `Secret<String>`. Without a schema there are no such keys, and a password
    /// can be read as an ordinary `String`.
    #[arg(
        id = "config_schema",
        long = "config-schema",
        value_name = "MODULE.FN",
        requires = "host"
    )]
    pub schema: Option<String>,
}

impl ConfigOptions {
    /// The sources, read once, before anything is bound.
    ///
    /// `None` without `--host`: no file is opened and the process environment is
    /// not consulted, whatever it holds.
    pub fn read(&self, host: bool) -> Result<Option<Sources>, Vec<Diagnostic>> {
        if !host {
            return Ok(None);
        }
        if let Some(name) = &self.schema {
            schema::check_shape(name).map_err(|d| vec![d])?;
        }
        Sources::read(&self.set, &self.files).map(Some)
    }
}

/// A run's configuration, resolved: the snapshot the handlers answer from, and
/// what the report may say about it.
#[derive(Clone, Default)]
pub struct Configuration {
    /// Shared with the `Host` that serves `config.get` and `config.secret`. One
    /// per run, immutable, and the reason two host-backed tests that read
    /// configuration are not coupled.
    pub snapshot: Arc<Snapshot>,
    /// The `--config-schema` function's name and the keys it declared, or `None`
    /// for a run that named none.
    pub schema: Option<SchemaView>,
}

/// What the run learned from the schema it named.
///
/// The key *names* and *shapes* are here and the resolved values are not: the
/// digest covers this, and a CI check that broke on a deployment's own
/// configuration is a CI check people learn to ignore.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SchemaView {
    pub name: String,
    pub keys: Vec<(String, Shape)>,
}

impl Configuration {
    /// Resolve everything: read the sources, materialise the schema, check every
    /// declared key against it.
    ///
    /// The warnings — `W0607` for a `--set` the schema does not declare — come
    /// back rather than being printed, because this crate's commands each own
    /// their own report and a warning printed here would land outside the
    /// `--json` document.
    pub fn open(
        program: &Program,
        resolved: &Resolved,
        check: &CheckOutput,
        host: bool,
        options: &ConfigOptions,
    ) -> Result<(Configuration, Vec<Diagnostic>), Vec<Diagnostic>> {
        let Some(sources) = options.read(host)? else {
            return Ok((Configuration::default(), Vec::new()));
        };
        let spec = match &options.schema {
            None => None,
            Some(name) => {
                Some(schema::materialise(program, resolved, check, name).map_err(|d| vec![d])?)
            }
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

    /// Whether this run opened any source at all. A hermetic run did not, and
    /// says so rather than printing a block of zeroes that reads like a run that
    /// was configured with nothing.
    pub fn is_opened(&self) -> bool {
        self.snapshot.has_spec()
            || self.snapshot.environment > 0
            || self.snapshot.sets > 0
            || !self.snapshot.files.is_empty()
    }

    /// The `configuration` block of `ply hosts --host`.
    ///
    /// Three lines, and every number in them is a fact the run already holds:
    /// the source counts are the snapshot's own, the schema line is the
    /// materialised spec, and the keys line is the resolution. Nothing is
    /// computed for the report.
    pub fn lines(&self) -> Vec<String> {
        let counts = self.snapshot.counts();
        let mut lines = vec![format!(
            "sources    --set {} · --config {} {} · environment {} · defaults {}",
            self.snapshot.sets,
            self.snapshot.files.len(),
            crate::commands::common::plural(self.snapshot.files.len(), "file"),
            self.snapshot.environment,
            counts.default,
        )];
        match &self.schema {
            None => lines.push(
                "schema     none — without `--config-schema` a missing key is a `None` at the \
                 call site, later and per key"
                    .to_string(),
            ),
            Some(view) => lines.push(format!(
                "schema     {} · {} {} · {} resolved · {} secret",
                view.name,
                view.keys.len(),
                crate::commands::common::plural(view.keys.len(), "key"),
                counts.keys,
                counts.secret,
            )),
        }
        if counts.keys > 0 {
            let keys: Vec<String> = self
                .snapshot
                .declared()
                .map(|(name, resolved)| {
                    format!("{name}={} ({})", resolved.shown(), resolved.source.as_str())
                })
                .collect();
            lines.push(format!("keys       {}", keys.join(" · ")));
        }
        lines
    }

    /// The one line the start-up banner carries.
    pub fn banner(&self) -> String {
        let counts = self.snapshot.counts();
        let mut parts = vec![format!(
            "{} {}",
            counts.keys,
            crate::commands::common::plural(counts.keys, "key")
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

    /// The `--json` object.
    ///
    /// The **keys and their sources** are here and a secret's value is not: an
    /// operator debugging "it used the wrong credential" needs to know which
    /// source won, and that is metadata rather than the value.
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

    /// Whether this configuration is part of what a CI check pins.
    ///
    /// Only a named `--config-schema` is. A run under `--host` always reads the
    /// process environment, so a digest that moved because a block *existed*
    /// would move on every `--host` run of every W4 program — and the one line a
    /// CI check pins may not depend on whether `--host` was passed.
    pub fn is_pinned(&self) -> bool {
        self.schema.is_some()
    }

    /// What the trusted-computing-base digest covers: the schema function's
    /// name, and every key's name and shape.
    ///
    /// Deliberately **not** the resolved values, the number of environment
    /// variables, or which source won. Those are a deployment's own
    /// configuration, and a digest that moved when a region changed would be a
    /// digest CI learns to ignore.
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

// --- `--config-schema` ------------------------------------------------------

/// Resolving `--config-schema <module>.<fn>` against the program, and reading
/// the value it returns.
///
/// A configuration schema is a value, exactly as a database schema is: there is
/// no separate schema file, no format to learn and nothing that can disagree
/// with the program, because the program is where it is written.
pub mod schema {
    use super::*;

    /// The type a `--config-schema` function must return, by simple name.
    /// Matched on the tail rather than on the whole program-wide name so that a
    /// project aliasing `std.config` still resolves.
    const SPEC_TYPE: &str = "ConfigSpec";

    /// The one field the pinned `ConfigSpec` record has.
    const KEYS: &str = "keys";

    /// `<module>.<fn>`, before any program is in hand.
    pub fn check_shape(name: &str) -> Result<(), Diagnostic> {
        let segments: Vec<&str> = name.split('.').collect();
        let well_formed = segments.len() >= 2
            && segments.iter().all(|s| {
                !s.is_empty()
                    && s.chars()
                        .next()
                        .is_some_and(|c| c.is_alphabetic() || c == '_')
                    && s.chars().all(|c| c.is_alphanumeric() || c == '_')
            });
        if well_formed {
            return Ok(());
        }
        Err(Diagnostic::error(
            codes::CONFIG_UNAVAILABLE,
            format!("`--config-schema {name}` is not a `<module>.<fn>` name"),
        )
        .primary(Span::DUMMY, "this argument is the run's configuration")
        .note("write the program-wide name of the function, as `ply hash` prints it"))
    }

    /// The definition `--config-schema` names, checked to be one a schema can be
    /// materialised from.
    ///
    /// Every refusal lists the candidates, because the fix is a different
    /// argument rather than an edit to the program, and an operator who mistyped
    /// a module prefix should not have to run a second command to find out what
    /// they meant.
    pub fn resolve<'a>(check: &'a CheckOutput, name: &str) -> Result<&'a Symbol, Diagnostic> {
        let Some((symbol, def)) = check.defs.iter().find(|(key, _)| key.as_str() == name) else {
            return Err(unknown(check, name));
        };
        let Type::Fn { params, ret, .. } = &def.scheme.ty else {
            return Err(not_a_spec_fn(
                name,
                "it is not a function, and a schema is materialised by calling one",
            ));
        };
        if !params.is_empty() {
            return Err(not_a_spec_fn(
                name,
                &format!(
                    "it takes {} argument{}, and the run has nothing to pass",
                    params.len(),
                    if params.len() == 1 { "" } else { "s" }
                ),
            ));
        }
        if !returns_spec(ret) {
            return Err(not_a_spec_fn(
                name,
                &format!("it returns `{ret}` rather than a `ConfigSpec`"),
            ));
        }
        if !def.footprint.is_empty() {
            return Err(not_a_spec_fn(
                name,
                &format!(
                    "its row is `{}`, and the schema is read before anything is bound, so it \
                     must be pure",
                    def.footprint
                ),
            ));
        }
        Ok(symbol)
    }

    /// Resolve, evaluate and decode.
    ///
    /// Unlike `--db-schema`, a failure to *evaluate* is a refusal rather than an
    /// absent count: this value decides which keys are required and which are
    /// credentials, so a run that could not compute it does not know whether it
    /// is configured.
    pub fn materialise(
        program: &Program,
        resolved: &Resolved,
        check: &CheckOutput,
        name: &str,
    ) -> Result<Spec, Diagnostic> {
        resolve(check, name)?;
        let def = check
            .defs
            .values()
            .find(|d| d.name.as_str() == name)
            .ok_or_else(|| unknown(check, name))?;
        let value = ply_eval::Machine::new(program, resolved, check)
            .call(name, Vec::new(), def.span)
            .map_err(|failure| {
                Diagnostic::error(
                    codes::CONFIG_UNAVAILABLE,
                    format!("`--config-schema {name}` could not be evaluated: {}", failure.message),
                )
                .primary(def.span, "this function decides what the run requires of its configuration")
                .note("it is called once, before anything is bound, and a run that cannot compute its schema does not know whether it is configured")
            })?;
        spec_of(&value, name)
    }

    /// `{ keys: List<Key> }`, structurally.
    ///
    /// The type checker already accepted the return type; what is checked here
    /// is that the *value* has the fields the resolution reads, because a
    /// `ConfigSpec` that decoded partially would silently drop a required key
    /// and turn `E0441` into the `None` at first use it exists to prevent.
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

    /// `ConfigSpec` is `{ keys: List<Key> }`, and a record type alias is
    /// **expanded** by inference — so by the time a signature is in hand there is
    /// no name left to match on and the check has to be structural. The nominal
    /// arm stays for a `ConfigSpec` that is an ADT or an opaque constructor
    /// rather than a record.
    fn returns_spec(ret: &Type) -> bool {
        match ret {
            Type::Con(name, args) if args.is_empty() => name
                .as_str()
                .rsplit('.')
                .next()
                .is_some_and(|tail| tail == SPEC_TYPE),
            Type::Record(fields) => {
                fields.len() == 1
                    && matches!(
                        fields.get(&Symbol::new(KEYS)),
                        Some(Type::Con(name, args)) if name.as_str() == "List" && args.len() == 1
                    )
            }
            _ => false,
        }
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

    fn not_a_spec_fn(name: &str, why: &str) -> Diagnostic {
        Diagnostic::error(
            codes::CONFIG_UNAVAILABLE,
            format!("`--config-schema {name}` does not name a configuration schema: {why}"),
        )
        .primary(Span::DUMMY, "this argument is the run's configuration")
        .note("it must be a nullary pure function returning `std.config.ConfigSpec` — a record `{keys: List<Key>}`")
    }

    fn unknown(check: &CheckOutput, name: &str) -> Diagnostic {
        let mut candidates: Vec<&str> = check
            .defs
            .iter()
            .filter(|(_, def)| match &def.scheme.ty {
                Type::Fn { params, ret, .. } => {
                    params.is_empty() && returns_spec(ret) && def.footprint.is_empty()
                }
                _ => false,
            })
            .map(|(key, _)| key.as_str())
            .collect();
        candidates.sort_unstable();

        let mut diagnostic = Diagnostic::error(
            codes::CONFIG_UNAVAILABLE,
            format!("`--config-schema {name}` names no definition in this program"),
        )
        .primary(Span::DUMMY, "this argument is the run's configuration");
        diagnostic = if candidates.is_empty() {
            diagnostic
                .note("this program declares no nullary function returning a `ConfigSpec`")
                .note("drop `--config-schema`: without it a missing key is a `None` at the call site, later and per key and still the program's to handle")
        } else {
            diagnostic.note(format!("this program has: {}", candidates.join(", ")))
        };
        diagnostic
    }
}

#[cfg(test)]
mod tests;
