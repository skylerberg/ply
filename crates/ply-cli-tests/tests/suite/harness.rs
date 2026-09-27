//! The one way a test drives `ply`.
//!
//! `ply` is a process, so a test's inputs are its arguments, its project on disk and its
//! environment. The environment is the one a test forgets: `ply` reads it as a host-configuration
//! source, hands it to the program through the environment binding, and takes its C toolchain,
//! emitter, profile and stage from `PLY_*` names. A variable in the developer's shell can
//! therefore decide what a test asserts, and a test that re-derives its own command gets that
//! hygiene wrong one file at a time. Every command here starts from [`INHERITED`] and nothing
//! else, so the environment a test sees is one it asked for.

use assert_cmd::Command;
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Output};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// What a `ply` child inherits from the test process.
///
/// `PATH` is how the C backend finds a compiler; the temporary-directory names are what
/// `PLY_C_STAGE` and `PLY_C_CACHE` default to, and those are content-addressed caches of the
/// emitter and the compiler -- shared deliberately, since a cache is not an input and a private
/// one per test would recompile the world 694 times. `HOME` is here for the same reason `PATH` is:
/// a toolchain that reads it should keep working. `NEXTEST` decides whether the emitter's own
/// unit compiles in the foreground, which is a property of the runner and not of the test.
const INHERITED: &[&str] = &["PATH", "HOME", "TMPDIR", "TEMP", "TMP", "NEXTEST"];

/// The `ply` binary under test. Cargo builds it for this package's tests only because
/// `crates/ply-launcher/tests/binary.rs` exists; without that, `CARGO_BIN_EXE_ply` is unset.
#[track_caller]
pub fn bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("ply")
}

/// A hermetic `ply` for a project at `dir`, for tests that only run it.
#[track_caller]
pub fn ply(dir: &Path) -> Command {
    Command::from_std(process(dir))
}

/// A hermetic `ply` for a project at `dir`, for tests that spawn it, wait on it or signal it.
#[track_caller]
pub fn process(dir: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(bin());
    cmd.env_clear();
    for key in INHERITED {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    // Before the test's own arguments, so one that spells `--color` itself wins.
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

/// A scratch directory for a command that reads no project, like `explain` or `hosts --digest`.
pub fn scratch() -> TempDir {
    TempDir::new().expect("a scratch directory")
}

/// A temporary project whose `m.ply` is `source`.
pub fn project(source: &str) -> TempDir {
    let dir = scratch();
    write(dir.path(), "m.ply", source);
    dir
}

/// A temporary project holding each `(name, source)` as a file.
pub fn project_files(files: &[(&str, &str)]) -> TempDir {
    let dir = scratch();
    for (name, source) in files {
        write(dir.path(), name, source);
    }
    dir
}

/// Writes `text` to `dir/name`, making the directories above it.
pub fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the file's directory is made");
    }
    std::fs::write(path, text).expect("the fixture is written");
}

/// The repository root. Canonical, so a path compared against `ply`'s output -- which resolves
/// its own roots -- is comparable on a machine whose temporary directory is a symlink.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate lives two levels below the repository root")
}

pub fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

pub fn stderr_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is utf-8")
}

/// The one JSON document `--json` writes on stdout.
#[track_caller]
pub fn json_of(output: &Output) -> Value {
    let text = stdout_of(output);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("`--json` writes one document on stdout: {e}\n{text}"))
}

/// A port for a `ply run --host` server a test is about to start, held against every other test
/// that reserves this way until the server has answered on it.
///
/// `bind(0)` returns a port to the kernel the moment the reserving listener drops, so two tests
/// that reserve together can be handed one port: the server that starts second dies with `E0502`
/// while the first answers the second's readiness probe. One lock, held across the
/// reserve-to-answer window rather than the reserve alone, keeps a sibling out of the gap in which
/// the port is neither reserved nor bound.
///
/// A server that can echo a token of its own -- the shutdown suite's -- proves itself by the answer
/// instead. The example servers serve a fixed number of connections and cannot spend one on a
/// probe, so for them the port itself has to be un-shareable.
pub struct Reservation {
    port: u16,
    _lock: File,
}

impl Reservation {
    /// Takes the run-wide lock, then a free port. Dropping the value releases both.
    pub fn take() -> Reservation {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_file())
            .expect("the reservation lock opens");
        lock.lock().expect("the reservation lock is taken");
        let listener = TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
        let port = listener.local_addr().expect("a bound address").port();
        drop(listener);
        Reservation { port, _lock: lock }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The server has answered on the port; the port is the server's now, so the reservation ends.
    pub fn bound(self) {}
}

/// Beside the test binary, so every test process in a run shares one lock whatever its `TMPDIR`.
fn lock_file() -> PathBuf {
    std::env::current_exe()
        .expect("the test binary's own path")
        .parent()
        .expect("the test binary lives in a directory")
        .join(".ply-ports.reserve")
}

/// Connects to `addr`, retrying until `ready` takes a connection or the child exits. `ready` has to
/// say why the answer is *this* server's: a bare connect can belong to another process that was
/// handed the same port. A test holding a [`Reservation`] can answer `|_| true`.
pub fn connect_when_ready(
    child: &mut Child,
    addr: SocketAddr,
    deadline: Duration,
    mut ready: impl FnMut(&mut TcpStream) -> bool,
) -> Result<TcpStream, String> {
    let until = Instant::now() + deadline;
    loop {
        if let Some(status) = child.try_wait().expect("the child is waitable") {
            return Err(format!(
                "`ply run --host` exited {status} before listening:\n{}",
                output_of(child)
            ));
        }
        if let Ok(mut probe) = TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
            let _ = probe.set_read_timeout(Some(Duration::from_secs(10)));
            if ready(&mut probe) {
                return Ok(probe);
            }
        }
        if Instant::now() >= until {
            return Err(format!("nothing listening on {addr} after {deadline:?}"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Everything a child that has exited wrote, for the failure it is about to explain.
fn output_of(child: &mut Child) -> String {
    let mut out = String::new();
    if let Some(stdout) = child.stdout.as_mut() {
        let _ = stdout.read_to_string(&mut out);
    }
    let mut err = String::new();
    if let Some(stderr) = child.stderr.as_mut() {
        let _ = stderr.read_to_string(&mut err);
    }
    format!("{out}{err}")
}
