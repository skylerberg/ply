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
pub mod bootstrap;
pub mod builder;
pub mod cache;
pub mod claims;
pub mod config;
pub mod costs;
pub mod drive;
pub mod driver;
pub mod edit;
pub mod engine;
pub mod hosts;
pub mod load;
pub mod migrate;
pub mod mutate;
pub mod options;
pub mod payload;
pub mod policy;
pub mod reused;
pub mod shelf;
pub mod simulation;
pub mod support;
pub mod tester;
pub mod trace;
pub mod vcs;
pub mod warm;

use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_eval::{Diagnostic, Span, Symbol, Value, codes};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

/// The effect a program declares to drive a machine: `machine.load[m](..)`, `machine.reuse[m](..)`,
/// `machine.bound[m](..)`, `machine.enter[m]()`, `machine.reload[m]()`, `machine.drop[m]()`.
pub const EFFECT: &str = "machine";

const OPERATIONS: [(&str, &str); 9] = [
    ("configure", "ply_machine::configure"),
    ("load", "ply_machine::load"),
    ("reuse", "ply_machine::reuse"),
    ("reload", "ply_machine::reload"),
    ("bound", "ply_machine::bound"),
    ("enter", "ply_machine::enter"),
    ("call", "ply_machine::call"),
    ("accounting", "ply_machine::accounting"),
    ("drop", "ply_machine::drop"),
];

/// The front end and the emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

/// The ops and the one handler serving them, configured as the run being lent is configured.
pub fn registrations_with(options: drive::RunOptions) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        options,
        labels: Mutex::new(HashMap::new()),
        configured: Mutex::new(HashMap::new()),
    });
    OPERATIONS
        .into_iter()
        .map(|(op, path)| (registration(op, path), Arc::clone(&site)))
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

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // A tree, a clock, a toolchain and a running program are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // A load may be bound and entered any number of times.
        linearity: Linearity::Repeatable,
        // The answer is in hand when the operation returns: this thread waits for the one the
        // machine lives on rather than being handed a token to poll.
        blocking: false,
        secrets: false,
        path,
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
    Bound {
        entry: String,
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
            ("reuse", [root, walked]) => self.reuse(&label, root, walked, span)?,
            ("reload", [front]) => {
                let front = Box::new(crate::driver::handed_front_of(front, span)?);
                let answer: Result<drive::FoundData, drive::Refused> =
                    self.ask(&label, span, |reply| Go::Reload { reply, front })?;
                match answer {
                    Ok(found) => ok(drive::found_value(&found)),
                    Err(refused) => refused_value(&refused),
                }
            }
            ("bound", [entry]) => {
                let entry = entry.as_str(span, "an entry point's name")?.to_string();
                let answer: Result<drive::Disclosed, drive::Refused> =
                    self.ask(&label, span, |reply| Go::Bound { entry, reply })?;
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
        let parsed = drive::run_options_of(options, span)?;
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
        let found = self.open(label, root, options, span)?;
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
            Ok(found) => ok(drive::found_value(&found)),
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
        match self.open(label, root, options, span)? {
            Ok(found) => {
                // A load that opens consumes its label's configuration.
                self.taken(label);
                ply_codegen::c::sweep::used(&entry);
                Ok(payload::option(Some(drive::found_value(&found))))
            }
            Err(_) => Ok(payload::option(None)),
        }
    }

    /// What the label was configured with, taken for the load that uses it.
    fn taken(&self, label: &str) -> drive::RunOptions {
        self.configured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(label)
            .unwrap_or_else(|| self.options.clone())
    }

    /// The target at `root` opened on a thread of its own, and parked there under `label` when it
    /// opens.
    fn open(
        &self,
        label: &str,
        root: String,
        options: drive::RunOptions,
        span: Span,
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
        let path = PathBuf::from(root);
        let thread = std::thread::Builder::new()
            .name(format!("machine-{label}"))
            .stack_size(STACK)
            .spawn(move || serve(options, path, reply, hearing))
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
    options: drive::RunOptions,
    root: PathBuf,
    reply: Sender<Result<drive::FoundData, drive::Refused>>,
    hearing: mpsc::Receiver<Go>,
) {
    match drive::Drive::open(options, &root) {
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
            Go::Bound { entry, reply } => {
                let _ = reply.send(drive.bound(&entry));
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
