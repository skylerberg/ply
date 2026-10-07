//! The `os` effect: what the machine and the process a program runs in say of themselves. None of
//! it reads the environment, so no variable a user sets changes an answer.

use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, Span, Symbol, Value, codes,
};
use std::sync::Arc;

pub const MODULE: &str = "std.os";

pub const EFFECT: &str = "std.os.os";

operations! {
    what "os";
    path "os";
    Family = "family",
    Name = "name",
    Arch = "arch",
    Cpus = "cpus",
    Hostname = "hostname",
    User = "user",
    Pid = "pid",
    Executable = "executable",
    Endian = "endian",
    PointerBits = "pointer_bits",
}

impl Op {
    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // A reading takes nothing, so reading it again changes nothing outside the program.
            linearity: Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: self.path(),
        }
    }
}

pub fn registrations() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    Op::ALL
        .iter()
        .map(|op| {
            let handler: Arc<dyn HostHandler> = Arc::new(Operation { op: *op });
            (op.declaration(), handler)
        })
        .collect()
}

pub fn register(registry: &mut HostRegistry) {
    for (op, handler) in registrations() {
        registry.register(op, handler);
    }
}

/// A constructor of `std.os` that carries nothing.
fn named(simple: &str) -> Value {
    Value::ctor(format!("{MODULE}.{simple}"), Vec::new())
}

/// The constructor `std.os` gives a family or an architecture it has no name for.
fn unlisted(simple: &str, text: &str) -> Value {
    Value::ctor(format!("{MODULE}.{simple}"), vec![Value::str(text)])
}

/// `std.os.Family`, from the name the toolchain gives the system this binary was built for.
pub fn family() -> Value {
    match std::env::consts::OS {
        "linux" => named("Linux"),
        "macos" => named("MacOs"),
        "windows" => named("Windows"),
        "freebsd" => named("FreeBsd"),
        "openbsd" => named("OpenBsd"),
        "netbsd" => named("NetBsd"),
        other => unlisted("OtherFamily", other),
    }
}

/// `std.os.Arch`, likewise.
pub fn arch() -> Value {
    match std::env::consts::ARCH {
        "x86_64" => named("X86_64"),
        "aarch64" => named("Aarch64"),
        "x86" => named("X86"),
        "arm" => named("Arm"),
        "riscv64" => named("Riscv64"),
        other => unlisted("OtherArch", other),
    }
}

/// The processors this process may run on at once: what its affinity mask and its control
/// group's quota leave of the machine's, where the platform says.
pub fn cpus() -> i64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as i64)
}

/// A C string a system call wrote into `bytes`, to its first NUL.
fn c_text(bytes: &[libc::c_char]) -> String {
    let text: Vec<u8> = bytes
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&text).into_owned()
}

/// The system's own name for itself and its release, as `uname -sr` prints them.
#[cfg(unix)]
pub fn name() -> std::io::Result<String> {
    // SAFETY: `said` is plain data the call only writes.
    let said = unsafe {
        let mut said: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut said) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        said
    };
    Ok(format!(
        "{} {}",
        c_text(&said.sysname),
        c_text(&said.release)
    ))
}

#[cfg(not(unix))]
pub fn name() -> std::io::Result<String> {
    Ok(std::env::consts::OS.to_string())
}

/// The name the kernel holds for this machine; no resolver is asked.
#[cfg(unix)]
pub fn hostname() -> std::io::Result<String> {
    let mut name = [0 as libc::c_char; 256];
    // SAFETY: the pointer and length are `name`'s, less the byte that keeps it terminated.
    if unsafe { libc::gethostname(name.as_mut_ptr(), name.len() - 1) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(c_text(&name))
}

#[cfg(not(unix))]
pub fn hostname() -> std::io::Result<String> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// The user this process runs as, and the name the system's own records give that id: `None`
/// where they hold no entry for it, as in a container run under an arbitrary id.
#[cfg(unix)]
pub fn user() -> (i64, Option<String>) {
    // SAFETY: a plain system call that cannot fail.
    let id = unsafe { libc::getuid() };
    let mut text = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: `entry` is plain data the call only writes, `text` backs the strings it points at
    // and outlives every read of them, and `found` is null unless the call filled `entry`.
    let name = unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let answered = libc::getpwuid_r(id, &mut entry, text.as_mut_ptr(), text.len(), &mut found);
        (answered == 0 && !found.is_null() && !entry.pw_name.is_null()).then(|| {
            std::ffi::CStr::from_ptr(entry.pw_name)
                .to_string_lossy()
                .into_owned()
        })
    };
    (i64::from(id), name)
}

#[cfg(not(unix))]
pub fn user() -> (i64, Option<String>) {
    (0, None)
}

/// The file this process is running, as an absolute path with its links followed, so two
/// processes of one program answer alike however each was started.
pub fn executable() -> Option<String> {
    let started_as = std::env::current_exe().ok()?;
    let path = started_as.canonicalize().ok()?;
    Some(path.to_string_lossy().into_owned())
}

fn option(value: Option<Value>) -> Value {
    match value {
        Some(value) => Value::ctor("Some", vec![value]),
        None => Value::ctor("None", Vec::new()),
    }
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

struct Operation {
    op: Op,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        if !req.args.is_empty() {
            return Err(arity(self.op, req.args.len(), req.span));
        }
        let unread = |e: std::io::Error| unread(self.op, &e, req.span);
        Ok(HostAnswer::Value(match self.op {
            Op::Family => family(),
            Op::Name => Value::str(name().map_err(unread)?),
            Op::Arch => arch(),
            Op::Cpus => Value::Int(cpus()),
            Op::Hostname => Value::str(hostname().map_err(unread)?),
            Op::User => {
                let (id, name) = user();
                record([
                    ("id", Value::Int(id)),
                    ("name", option(name.map(Value::str))),
                ])
            }
            Op::Pid => Value::Int(i64::from(std::process::id())),
            Op::Executable => option(executable().map(Value::str)),
            Op::Endian => named(if cfg!(target_endian = "big") {
                "Big"
            } else {
                "Little"
            }),
            Op::PointerBits => Value::Int(i64::from(usize::BITS)),
        }))
    }
}

#[cold]
fn unread(op: Op, e: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{} could not be read: {e}", op.what()),
    )
    .primary(span, "the system did not answer")
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{} was performed with {got} arguments and takes none", op.what()),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}
