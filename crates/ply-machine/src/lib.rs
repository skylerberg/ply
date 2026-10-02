//! Programs entering programs: the nested-entry capability, as host handlers.
//!
//! The `ply` program's commands run other Ply programs — `run` enters one, `test` and `prove`
//! evaluate bodies, `build` enters the emitter. A compiled body entered while another entry holds
//! the thread is declined, so the machine these operations drive lives on a thread of its own per
//! resource label, parked on a channel between operations: `load[m]` opens the target rooted at a
//! path (`reload[m]` asks again, and `reuse[m]` opens it over the front end an earlier run filed),
//! `bound[m]` binds the hosts the named entry may reach and answers the disclosure, `enter[m]`
//! runs it and answers how it ended, `drop[m]` lets the thread go. The run flow itself —
//! targets, bindings, teardown — is `crate::drive`.

pub mod artifact;
pub mod body;
pub mod bootstrap;
pub mod builder;
pub mod claims;
pub mod config;
pub mod costs;
pub mod drive;
pub mod driver;
pub mod edit;
pub mod engine;
pub mod hosts;
pub mod load;
pub mod options;
pub mod payload;
pub mod policy;
pub mod recording;
pub mod reused;
pub mod shelf;
pub mod shipped;
pub mod support;
pub mod tester;
pub mod testrun;
pub mod trace;
pub mod vcs;

use ply_eval::host::{
    HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostRuntime, Linearity,
};
use ply_eval::{Diagnostic, Span, Value, codes};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

/// The effect a program declares to drive a machine: `machine.load[m](..)`, `machine.reuse[m](..)`,
/// `machine.bound[m](..)`, `machine.enter[m]()`, `machine.reload[m]()`, `machine.drop[m]()`.
pub const EFFECT: &str = "machine";

pub const HERMETIC: &str = "hermetic_machine";

const OPERATIONS: [(&str, &str); 11] = [
    ("configure", "ply_machine::configure"),
    ("load", "ply_machine::load"),
    ("opened", "ply_machine::opened"),
    ("reuse", "ply_machine::reuse"),
    ("reload", "ply_machine::reload"),
    ("schema", "ply_machine::schema"),
    ("bound", "ply_machine::bound"),
    ("enter", "ply_machine::enter"),
    ("call", "ply_machine::call"),
    ("accounting", "ply_machine::accounting"),
    ("drop", "ply_machine::drop"),
];

const HERMETIC_OPERATIONS: [(&str, &str); 11] = [
    ("configure", "ply_machine::hermetic::configure"),
    ("load", "ply_machine::hermetic::load"),
    ("opened", "ply_machine::hermetic::opened"),
    ("reuse", "ply_machine::hermetic::reuse"),
    ("reload", "ply_machine::hermetic::reload"),
    ("schema", "ply_machine::hermetic::schema"),
    ("bound", "ply_machine::hermetic::bound"),
    ("enter", "ply_machine::hermetic::enter"),
    ("call", "ply_machine::hermetic::call"),
    ("accounting", "ply_machine::hermetic::accounting"),
    ("drop", "ply_machine::hermetic::drop"),
];

/// The front end and the emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

/// The ops and the one handler serving them, configured as the run being lent is configured.
pub fn registrations_with(options: drive::RunOptions) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    registrations_in(EFFECT, options)
}

/// The machine for a program that declares `machine` in `module`, which is where `Target` is
/// declared too: what a load answers crosses under that module's name.
pub fn registrations_in(
    module: &str,
    options: drive::RunOptions,
) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    lent_by(module, options, false)
}

/// A machine that loads only what it is handed, binds no host, reads no clock and files nothing.
pub fn hermetic_registrations_in(module: &str) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let options = drive::RunOptions {
        hermetic: true,
        ..drive::RunOptions::default()
    };
    lent_by(module, options, true)
}

fn lent_by(
    module: &str,
    options: drive::RunOptions,
    hermetic: bool,
) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        module: module.to_string(),
        hermetic,
        options,
        labels: Mutex::new(HashMap::new()),
        configured: Mutex::new(HashMap::new()),
    });
    let (effect, operations) = if hermetic {
        (HERMETIC, HERMETIC_OPERATIONS)
    } else {
        (EFFECT, OPERATIONS)
    };
    // A load may be bound and entered any number of times.
    operations
        .into_iter()
        .map(|(op, path)| {
            let op = if hermetic {
                hosts::hermetic_op(effect, op, Linearity::Repeatable, path)
            } else {
                hosts::privileged_op(effect, op, Linearity::Repeatable, path)
            };
            (op, Arc::clone(&site))
        })
        .collect()
}

/// Hermetic, unbounded, nothing on disk: the capability as a test or a tool defaults it.
pub fn registrations() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    registrations_with(drive::RunOptions::default())
}

pub fn register(registry: &mut HostRegistry) {
    for (op, handler) in registrations() {
        registry.register(op, handler);
    }
}

pub fn register_with(registry: &mut HostRegistry, options: drive::RunOptions) {
    for (op, handler) in registrations_with(options) {
        registry.register(op, handler);
    }
}

fn ok(value: Value) -> Value {
    Value::ctor("Ok", vec![value])
}

fn err(value: Value) -> Value {
    Value::ctor("Err", vec![value])
}

fn refused_value(refused: &drive::Refused) -> Value {
    err(drive::refusal_value(refused))
}

// --- The handler ------------------------------------------------------------------

struct Site {
    module: String,
    hermetic: bool,
    options: drive::RunOptions,
    labels: Mutex<HashMap<String, Labelled>>,
    /// What a label was configured with before it loaded, if it was.
    configured: Mutex<HashMap<String, drive::RunOptions>>,
}

/// A label's machine: the channel to its thread, and the join on the way out.
struct Labelled {
    go: Option<Sender<Go>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Labelled {
    fn join(&mut self) {
        // The sender goes first: the thread is parked on it, and dropping it ends the wait, so a
        // program that never drops its machine still leaves nothing running.
        self.go.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Labelled {
    fn drop(&mut self) {
        self.join();
    }
}

/// The steps' answers cross as plain data; a `Value` is not `Send`, so the handler thread builds
/// the one the program reads.
enum Go {
    Schema {
        name: String,
        reply: Sender<Result<ply_eval::Plain, Diagnostic>>,
    },
    Bound {
        entry: String,
        configuration: crate::config::Configuration,
        reply: Sender<Result<drive::Disclosed, drive::Refused>>,
    },
    Enter {
        reply: Sender<drive::Outcome>,
    },
    Call {
        name: String,
        args: Vec<ply_eval::Plain>,
        reply: Sender<ply_eval::Ended<ply_eval::Plain>>,
    },
    Accounting {
        reply: Sender<drive::Measured>,
    },
    Reload {
        front: Box<crate::driver::HandedFront>,
        reply: Sender<Result<drive::FoundData, drive::Refused>>,
    },
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let label = label_of(req, span)?;
        let value = match (req.op.op.as_str(), req.args) {
            ("configure", [options]) => self.configure(&label, options, span)?,
            ("load", [root, front, keep]) => self.load(&label, root, front, keep, span)?,
            ("opened", [path, bytes]) => self.opened(&label, path, bytes, span)?,
            ("reuse", [root, walked]) => self.reuse(&label, root, walked, span)?,
            ("reload", [front]) => {
                let front = Box::new(crate::driver::handed_front_of(front, span)?);
                let answer: Result<drive::FoundData, drive::Refused> =
                    self.ask(&label, span, |reply| Go::Reload { reply, front })?;
                match answer {
                    Ok(found) => ok(drive::found_value(&found, &self.module)),
                    Err(refused) => refused_value(&refused),
                }
            }
            ("schema", [name]) => {
                let name = name.as_str(span, "a definition's name")?.to_string();
                let answer: Result<ply_eval::Plain, Diagnostic> =
                    self.ask(&label, span, |reply| Go::Schema { name, reply })?;
                crate::config::schema_answer(answer)
            }
            ("bound", [entry, config]) => {
                let entry = entry.as_str(span, "an entry point's name")?.to_string();
                let configuration = crate::config::Configuration::of(config, span)?;
                let answer: Result<drive::Disclosed, drive::Refused> =
                    self.ask(&label, span, |reply| Go::Bound {
                        entry,
                        configuration,
                        reply,
                    })?;
                match answer {
                    Ok(disclosed) => ok(drive::disclosed_value(&disclosed)),
                    Err(refused) => refused_value(&refused),
                }
            }
            ("enter", []) => {
                let outcome: drive::Outcome =
                    self.ask(&label, span, |reply| Go::Enter { reply })?;
                drive::outcome_value(&outcome)
            }
            ("call", [name, args]) => {
                let name = name.as_str(span, "a definition's name")?.to_string();
                let Value::List(args) = args else {
                    return Err(Diagnostic::error(
                        codes::RUNTIME_ERROR,
                        "`machine.call`'s arguments are not a list".to_string(),
                    )
                    .primary(span, "a `std.value.Value` list"));
                };
                // Plain data, not the value: the call crosses to the machine's thread, and
                // runtime values do not cross threads.
                let args: Vec<ply_eval::Plain> = args
                    .iter()
                    .map(|a| crate::payload::value_plain(a, span))
                    .collect::<Result<_, _>>()?;
                let called: ply_eval::Ended<ply_eval::Plain> =
                    self.ask(&label, span, |reply| Go::Call { name, args, reply })?;
                drive::called_value(called)
            }
            ("accounting", []) => {
                let measured: drive::Measured =
                    self.ask(&label, span, |reply| Go::Accounting { reply })?;
                drive::accounting_value(&measured)
            }
            ("drop", []) => self.drop(&label),
            (other, _) => {
                return Err(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("`machine.{other}` is not an operation the machine serves"),
                )
                .primary(span, "this is Ply's fault"));
            }
        };
        Ok(HostAnswer::Value(value))
    }
}

fn label_of(req: &HostRequest<'_>, span: Span) -> Result<String, Diagnostic> {
    match &req.atom.resource {
        ply_eval::Resource::Named(name) => Ok(name.to_string()),
        _ => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "`machine` operations name a label: `machine.load[m](..)`".to_string(),
        )
        .primary(
            span,
            "a machine's label is the label its later operations answer for",
        )),
    }
}

impl Site {
    /// The program parsed the line; the machine reads the record.
    fn configure(&self, label: &str, options: &Value, span: Span) -> Result<Value, Diagnostic> {
        let mut parsed = drive::run_options_of(options, span)?;
        if self.hermetic {
            // No clock bounds a hermetic run: its step budget does.
            parsed.hermetic = true;
            parsed.timeout = 0;
        }
        self.configured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(label.to_string(), parsed);
        Ok(Value::Unit)
    }

    /// `keep` is the key the front is filed under once the load holds, for a later run of the same
    /// closure to take in place of its front end.
    fn load(
        &self,
        label: &str,
        root: &Value,
        front: &Value,
        keep: &Value,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let root = root.as_str(span, "the program's root")?.to_string();
        // `None` is a program loading a program of its own, at a root it chose while running.
        let handed = crate::payload::option_of(front, "a front end", span)?;
        let front = handed
            .map(|front| crate::driver::handed_front_of(front, span))
            .transpose()?;
        let keep = crate::payload::option_of(keep, "a key", span)?
            .filter(|_| !self.hermetic)
            .map(|key| key.as_str(span, "a key"))
            .transpose()?;
        // Taken before the front leaves for the machine's thread; the dump stays here as it came.
        let kept = keep.zip(front.as_ref()).map(|(key, front)| {
            let placed: Vec<(String, String)> = front
                .files
                .iter()
                .map(|f| (f.path.clone(), f.name.clone()))
                .collect();
            (key, placed)
        });
        let mut options = self.taken(label);
        options.front = front;
        let path = PathBuf::from(root);
        let found = self.open(label, options, span, move |options| {
            drive::Drive::open(options, &path)
        })?;
        if let (Ok(drive::FoundData::Project { .. }), Some((key, placed)), Some(handed)) =
            (&found, kept, handed)
        {
            crate::reused::file(
                key,
                &placed,
                crate::payload::field_of(handed, "dump", span)?,
            );
        }
        Ok(match found {
            Ok(found) => ok(drive::found_value(&found, &self.module)),
            Err(refused) => refused_value(&refused),
        })
    }

    /// The load an earlier run filed under the walk's key, over this run's own files, or `None`
    /// when nothing readable is filed there: then the label is left as it was, configuration and
    /// all, for the load the caller makes over a front end of its own.
    fn reuse(
        &self,
        label: &str,
        root: &Value,
        walked: &Value,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        use crate::payload::field_of;
        if self.hermetic {
            return Ok(payload::option(None));
        }
        let root = root.as_str(span, "the program's root")?.to_string();
        let key = field_of(walked, "key", span)?.as_str(span, "a key")?;
        let files = |name: &str| -> Result<Vec<reused::Walked>, Diagnostic> {
            field_of(walked, name, span)?
                .as_list(span, name)?
                .iter()
                .map(|file| {
                    Ok(reused::Walked {
                        path: field_of(file, "path", span)?
                            .as_str(span, "a path")?
                            .to_string(),
                        text: String::from_utf8_lossy(
                            field_of(file, "text", span)?.as_bytes(span, "a text")?,
                        )
                        .into_owned(),
                    })
                })
                .collect()
        };
        let Some((front, entry)) = reused::front(key, files("modules")?, files("manifests")?)
        else {
            return Ok(payload::option(None));
        };
        let mut options = self
            .configured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(label)
            .cloned()
            .unwrap_or_else(|| self.options.clone());
        options.front = Some(front);
        let path = PathBuf::from(root);
        match self.open(label, options, span, move |options| {
            drive::Drive::open(options, &path)
        })? {
            Ok(found) => {
                // A load that opens consumes its label's configuration.
                self.taken(label);
                ply_codegen::c::sweep::used(&entry);
                Ok(payload::option(Some(drive::found_value(
                    &found,
                    &self.module,
                ))))
            }
            Err(_) => Ok(payload::option(None)),
        }
    }

    /// `bytes` is `None` when nothing could be read at `path`.
    fn opened(
        &self,
        label: &str,
        path: &Value,
        bytes: &Value,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let path = PathBuf::from(path.as_str(span, "the artifact's path")?);
        let bytes = crate::payload::option_of(bytes, "the artifact's bytes", span)?
            .map(|b| b.as_bytes(span, "the artifact's bytes").map(|b| b.to_vec()))
            .transpose()?;
        let options = self.taken(label);
        let found = self.open(label, options, span, move |options| {
            drive::Drive::open_artifact(options, &path, bytes.as_deref())
        })?;
        Ok(match found {
            Ok(found) => ok(drive::found_value(&found, &self.module)),
            Err(refused) => refused_value(&refused),
        })
    }

    /// What the label was configured with, taken for the load that uses it.
    fn taken(&self, label: &str) -> drive::RunOptions {
        self.configured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(label)
            .unwrap_or_else(|| self.options.clone())
    }

    /// The target `start` opens, on a thread of its own, and parked there under `label` when it
    /// opens.
    fn open(
        &self,
        label: &str,
        options: drive::RunOptions,
        span: Span,
        start: impl FnOnce(drive::RunOptions) -> Result<drive::Drive, drive::Refused> + Send + 'static,
    ) -> Result<Result<drive::FoundData, drive::Refused>, Diagnostic> {
        if self
            .labels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(label)
        {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("`machine.load[{label}]` while that label already holds a program"),
            )
            .primary(span, "drop it first, or load under another label"));
        }
        let (reply, answered) = mpsc::channel();
        let (go, hearing) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name(format!("machine-{label}"))
            .stack_size(STACK)
            .spawn(move || serve(start(options), reply, hearing))
            .map_err(|e| unspawned(label, &e))?;
        let found = answered.recv().map_err(|_| unanswered(label))?;
        if found.is_err() {
            // The target refused: the thread has already said what it had to and is done.
            let _ = thread.join();
            return Ok(found);
        }
        self.labels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                label.to_string(),
                Labelled {
                    go: Some(go),
                    thread: Some(thread),
                },
            );
        Ok(found)
    }

    /// One round trip to the machine's thread: send the step, wait for the answer it sends back.
    fn ask<T: Send>(
        &self,
        label: &str,
        span: Span,
        go: impl FnOnce(Sender<T>) -> Go,
    ) -> Result<T, Diagnostic> {
        let (reply, answered) = mpsc::channel();
        {
            let labels = self.labels.lock().unwrap_or_else(|e| e.into_inner());
            let Some(machine) = labels.get(label) else {
                return Err(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("`machine` was asked about `{label}` before `machine.load[{label}]`"),
                )
                .primary(span, "a machine answers for the program it was loaded with"));
            };
            machine
                .go
                .as_ref()
                .ok_or_else(|| unanswered(label))?
                .send(go(reply))
                .map_err(|_| unanswered(label))?;
        }
        answered.recv().map_err(|_| unanswered(label))
    }

    fn drop(&self, label: &str) -> Value {
        let mut labels = self.labels.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut machine) = labels.remove(label) {
            machine.join();
        }
        Value::Unit
    }
}

fn unspawned(label: &str, e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the machine for `{label}` could not be started on a thread of its own: {e}"),
    )
    .primary(Span::DUMMY, "this is Ply's fault")
}

fn unanswered(label: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the thread this machine's `{label}` lives on stopped without answering"),
    )
    .primary(Span::DUMMY, "this is Ply's fault")
}

// --- The thread the machine lives on -----------------------------------------------

fn serve(
    opened: Result<drive::Drive, drive::Refused>,
    reply: Sender<Result<drive::FoundData, drive::Refused>>,
    hearing: mpsc::Receiver<Go>,
) {
    match opened {
        Ok(drive) => {
            let found = drive.found_data();
            let _ = reply.send(Ok(found));
            park(drive, hearing);
        }
        Err(refused) => {
            let _ = reply.send(Err(refused));
        }
    }
}

fn park(mut drive: drive::Drive, hearing: mpsc::Receiver<Go>) {
    while let Ok(go) = hearing.recv() {
        match go {
            Go::Schema { name, reply } => {
                let _ = reply.send(drive.schema(&name));
            }
            Go::Bound {
                entry,
                configuration,
                reply,
            } => {
                let _ = reply.send(drive.bound(&entry, configuration));
            }
            Go::Enter { reply } => {
                let _ = reply.send(drive.enter());
            }
            Go::Call { name, args, reply } => {
                let _ = reply.send(drive.call(&name, args));
            }
            Go::Accounting { reply } => {
                let _ = reply.send(drive.accounting());
            }
            Go::Reload { reply, front } => {
                let answer = drive.reload(&front).map(|()| drive.found_data());
                let _ = reply.send(answer);
            }
        }
    }
}
