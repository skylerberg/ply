//! Programs entering programs: the nested-entry capability, as host handlers.
//!
//! The `ply` program's commands run other Ply programs — `run` enters one, `test` and `prove`
//! evaluate bodies, `build` enters the emitter. A compiled body entered while another entry holds
//! the thread is declined, so the machine these operations drive lives on a thread of its own per
//! resource label, parked on a channel between operations: `load[m]` fronts the program and
//! answers what it found, `enter[m]` runs one of its entries and answers how it ended, `drop[m]`
//! lets the thread go.
//!
//! A nested run is hermetic but for `process`, whose lines are captured into the answer rather
//! than written: a program's output is its caller's to place.

use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_eval::{Provider, Value};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

/// The effect a program declares to drive a machine: `machine.load[m](..)`, `machine.enter[m](..)`,
/// `machine.drop[m]()`.
pub const EFFECT: &str = "machine";

const OPERATIONS: [(&str, &str); 3] = [
    ("load", "ply_machine::load"),
    ("enter", "ply_machine::enter"),
    ("drop", "ply_machine::drop"),
];

/// The front end and the emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

/// The ops and the one handler serving them, for a registry or a caller lending them.
pub fn registrations() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let site: Arc<dyn HostHandler> = Arc::new(Site::default());
    OPERATIONS
        .into_iter()
        .map(|(op, path)| (registration(op, path), Arc::clone(&site)))
        .collect()
}

pub fn register(registry: &mut HostRegistry) {
    for (op, handler) in registrations() {
        registry.register(op, handler);
    }
}

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // A front end, a toolchain and a running program are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // A load may be entered any number of times.
        linearity: Linearity::Repeatable,
        // The answer is in hand when the operation returns: this thread waits for the one the
        // machine lives on rather than being handed a token to poll.
        blocking: false,
        secrets: false,
        path,
    }
}

// --- The values that cross --------------------------------------------------------

// `Value::Record` holds an `Arc`, and its fields are not `Send`; the values are built on the
// calling thread and never cross one.
#[allow(clippy::arc_with_non_send_sync)]
fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

fn strings(items: impl IntoIterator<Item = String>) -> Value {
    Value::list(items.into_iter().map(Value::str).collect())
}

fn option(value: Option<Value>) -> Value {
    match value {
        Some(value) => Value::ctor("Some", vec![value]),
        None => Value::ctor("None", Vec::new()),
    }
}

fn ok(value: Value) -> Value {
    Value::ctor("Ok", vec![value])
}

fn err(value: Value) -> Value {
    Value::ctor("Err", vec![value])
}

fn field<'a>(value: &'a Value, name: &str, span: Span) -> Result<&'a Value, Diagnostic> {
    let Value::Record(fields) = value else {
        return Err(shape(value, span, "a record"));
    };
    fields
        .iter()
        .find(|(key, _)| key.as_str() == name)
        .map(|(_, value)| value)
        .ok_or_else(|| shape(value, span, &format!("a record with `{name}`")))
}

fn shape(value: &Value, span: Span, what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "`machine` was handed {} where {what} was expected",
            value.type_name()
        ),
    )
    .primary(
        span,
        "the program and the machine it drives are written together; this is Ply's fault",
    )
}

// --- The handler --------------------------------------------------------

#[derive(Default)]
struct Site {
    labels: Mutex<HashMap<String, Labelled>>,
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

enum Go {
    Enter {
        entry: String,
        argv: Vec<String>,
        reply: Sender<Entered>,
    },
}

/// What an entry came to, as plain data: the `Value` is built on the calling thread, since a
/// `Value` is not `Send`.
struct Entered {
    out: Vec<String>,
    err: Vec<String>,
    exit: Option<i32>,
    value: Option<String>,
    raised: Option<String>,
}

struct Loaded {
    modules: Vec<String>,
    definitions: usize,
    tests: usize,
}

enum Opened {
    Ready(Box<Loaded>),
    Refused(Vec<String>),
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let label = label_of(req, span)?;
        let value = match (req.op.op.as_str(), req.args) {
            ("load", [modules]) => self.load(&label, modules, span)?,
            ("enter", [entry, argv]) => self.enter(&label, entry, argv, span)?,
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
        ply_ty::Resource::Named(name) => Ok(name.to_string()),
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
    fn load(&self, label: &str, modules: &Value, span: Span) -> Result<Value, Diagnostic> {
        let mut own = Vec::new();
        for module in modules.as_list(span, "the program's modules")?.iter() {
            let name = field(module, "name", span)?.as_str(span, "a module's name")?;
            let text = field(module, "text", span)?.as_str(span, "a module's text")?;
            own.push((name.to_string(), text.to_string()));
        }
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
            .spawn(move || serve(own, reply, hearing))
            .map_err(|e| unspawned(label, &e))?;
        let opened = answered.recv().map_err(|_| unanswered(label))?;
        let value = match opened {
            Opened::Ready(loaded) => {
                let mut labels = self.labels.lock().unwrap_or_else(|e| e.into_inner());
                // The double-load was refused above, so this replaces nothing.
                let previous = labels.insert(
                    label.to_string(),
                    Labelled {
                        go: Some(go),
                        thread: Some(thread),
                    },
                );
                drop(previous);
                ok(record(vec![
                    ("modules", strings(loaded.modules)),
                    ("definitions", Value::Int(loaded.definitions as i64)),
                    ("tests", Value::Int(loaded.tests as i64)),
                ]))
            }
            Opened::Refused(diagnostics) => {
                err(record(vec![("diagnostics", strings(diagnostics))]))
            }
        };
        Ok(value)
    }

    fn enter(
        &self,
        label: &str,
        entry: &Value,
        argv: &Value,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let entry = entry.as_str(span, "an entry point's name")?.to_string();
        let argv: Vec<String> = argv
            .as_list(span, "the program's argument vector")?
            .iter()
            .map(|arg| arg.as_str(span, "an argument").map(str::to_string))
            .collect::<Result<Vec<String>, Diagnostic>>()?;
        let (reply, answered) = mpsc::channel();
        {
            let labels = self.labels.lock().unwrap_or_else(|e| e.into_inner());
            let Some(machine) = labels.get(label) else {
                return Err(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("`machine.enter[{label}]` before `machine.load[{label}]`"),
                )
                .primary(span, "a machine answers for the program it was loaded with"));
            };
            machine
                .go
                .as_ref()
                .ok_or_else(|| unanswered(label))?
                .send(Go::Enter { entry, argv, reply })
                .map_err(|_| unanswered(label))?;
        }
        let ended = answered.recv().map_err(|_| unanswered(label))?;
        Ok(record(vec![
            ("out", strings(ended.out)),
            ("err", strings(ended.err)),
            (
                "exit",
                option(ended.exit.map(|code| Value::Int(i64::from(code)))),
            ),
            ("value", option(ended.value.map(Value::str))),
            ("raised", option(ended.raised.map(Value::str))),
        ]))
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

/// The loaded program: a front end and its module texts, plus the compiled tier, built lazily on
/// the first entry and reused after.
struct Machine {
    front: ply_ty::Front,
    texts: HashMap<String, String>,
    unit: Option<&'static ply_codegen::Unit>,
}

fn serve(own: Vec<(String, String)>, reply: Sender<Opened>, hearing: mpsc::Receiver<Go>) {
    match load(&own) {
        Ok((machine, loaded)) => {
            let _ = reply.send(Opened::Ready(Box::new(loaded)));
            park(machine, hearing);
        }
        Err(diagnostics) => {
            let _ = reply.send(Opened::Refused(diagnostics));
        }
    }
}

fn park(mut machine: Machine, hearing: mpsc::Receiver<Go>) {
    while let Ok(go) = hearing.recv() {
        match go {
            Go::Enter { entry, argv, reply } => {
                let _ = reply.send(machine.enter(&entry, argv));
            }
        }
    }
}

fn load(own: &[(String, String)]) -> Result<(Machine, Loaded), Vec<String>> {
    let shelf: Vec<(String, String)> = ply_std::sources()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    ply_codegen::c::producer::ensure_default();
    let pulled = ply_codegen::c::producer::front_pulling_std(own, &shelf)
        .map_err(|e| vec![format!("the front end did not answer: {e:#}")])?;
    let mut sources = SourceMap::new();
    let mut ids = Vec::new();
    for (name, text) in own {
        ids.push(sources.add(PathBuf::from(name), text.clone()));
    }
    for module in &pulled.modules {
        let name = ply_ty::ModuleName::from_dotted(module);
        let text = ply_std::source(&name).unwrap_or_default();
        ids.push(sources.add(ply_std::pseudo_path(&name), text.to_string()));
    }
    let front = ply_ty::read_front(&pulled.dump, &ids)
        .map_err(|e| vec![format!("the front end's answer does not read: {e}")])?;
    let errors: Vec<String> = front
        .diagnostics
        .iter()
        .filter(|d| d.severity == ply_span::Severity::Error)
        .map(|d| ply_span::render::to_terminal(d, &sources, false))
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let loaded = Loaded {
        modules: own.iter().map(|(name, _)| name.clone()).collect(),
        definitions: front.check.defs.len(),
        tests: front.check.tests.len(),
    };
    let texts: HashMap<String, String> = front
        .check
        .modules
        .values()
        .filter_map(|m| {
            sources
                .get(m.source)
                .map(|f| (m.name.to_string(), f.text.to_string()))
        })
        .collect();
    Ok((
        Machine {
            front,
            texts,
            unit: None,
        },
        loaded,
    ))
}

impl Machine {
    fn enter(&mut self, entry: &str, argv: Vec<String>) -> Entered {
        let spec = ply_eval::BackendSpec {
            kind: ply_eval::BackendKind::C,
        };
        if self.unit.is_none() {
            match ply_codegen::Unit::over_front(&self.front, self.texts.clone()) {
                Ok(unit) => self.unit = Some(unit),
                Err(e) => {
                    return Entered {
                        out: Vec::new(),
                        err: Vec::new(),
                        exit: None,
                        value: None,
                        raised: Some(format!("the program has no compiled tier: {e:#}")),
                    };
                }
            }
        }
        let mut machine = ply_eval::Machine::new(&self.front);
        if let Some(unit) = &self.unit {
            machine.set_compiled(unit.attach(&spec));
        }
        let process = Arc::new(ply_host::process::ProcessHost::new(
            argv,
            ply_host::process::Sink::captured(),
        ));
        let mut registry = HostRegistry::new();
        ply_host::process::register(&mut registry, Some(&process));
        let binding = match registry.bind(&self.front.check) {
            Ok(binding) => binding,
            Err(diagnostics) => {
                return Entered {
                    out: Vec::new(),
                    err: Vec::new(),
                    exit: None,
                    value: None,
                    raised: Some(
                        diagnostics
                            .iter()
                            .map(|d| d.message.clone())
                            .collect::<Vec<String>>()
                            .join("\n"),
                    ),
                };
            }
        };
        machine.set_host_binding(Arc::new(binding));
        let span = self
            .front
            .check
            .defs
            .get(&Symbol::new(entry))
            .map(|d| d.span)
            .unwrap_or(Span::DUMMY);
        let answer = machine.call(entry, Vec::new(), span);
        let (out, err) = split(process.captured());
        let (value, raised) = match answer {
            Ok(value) => (Some(value.to_string()), None),
            // `process.exit` unwinds with its code recorded on the host: an answer, not a raise.
            Err(diagnostic) if diagnostic.code == codes::PROCESS_EXIT => (None, None),
            Err(diagnostic) => (None, Some(diagnostic.message)),
        };
        Entered {
            out,
            err,
            exit: process.requested_exit(),
            value,
            raised,
        }
    }
}

fn split(lines: Vec<(ply_host::process::Stream, String)>) -> (Vec<String>, Vec<String>) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    for (stream, line) in lines {
        match stream {
            ply_host::process::Stream::Out => out.push(line),
            ply_host::process::Stream::Err => err.push(line),
        }
    }
    (out, err)
}
