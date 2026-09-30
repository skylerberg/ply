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

/// What `ply check --explain --json` answers about the program at `dir`, less what two runs of one
/// program may differ by: where the project is, what each phase cost and where its answer came
/// from, and the cache's own warnings.
#[track_caller]
pub fn check_answer(dir: &Path) -> Value {
    let mut answer = json_of(
        &ply(dir)
            .args(["check", "--explain", "--json"])
            .output()
            .expect("`ply check` runs"),
    );
    let object = answer.as_object_mut().expect("one object");
    object.remove("root");
    object.remove("files");
    object.remove("front_end");
    if let Some(Value::Array(modules)) = object.get_mut("modules") {
        for module in modules.iter_mut() {
            if let Some(module) = module.as_object_mut() {
                module.remove("file");
            }
        }
    }
    if let Some(Value::Array(diagnostics)) = object.get_mut("diagnostics") {
        use ply_eval::codes;
        let about_the_cache = [
            codes::CACHE_UNREADABLE,
            codes::CACHE_CORRUPT,
            codes::CACHE_VERSION_CHANGED,
            codes::STDLIB_CHANGED,
        ];
        diagnostics.retain(|d| !about_the_cache.contains(&d["code"].as_str().unwrap_or("")));
    }
    answer
}

/// Every source under `from`, and nothing its caches hold, copied to `to`.
pub fn copy_sources(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from)
        .expect("the project is readable")
        .flatten()
    {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            std::fs::create_dir_all(to.join(&name)).expect("the copy's directory is made");
            copy_sources(&path, &to.join(&name));
        } else {
            std::fs::copy(&path, to.join(&name)).expect("the source is copied");
        }
    }
}

/// The program at `dir` checked from nothing: its sources, beside no cache.
#[track_caller]
pub fn cold_answer(dir: &Path) -> Value {
    let copy = scratch();
    copy_sources(dir, copy.path());
    check_answer(copy.path())
}

/// A check at `dir`, seeded from and filing into its cache, answers what a cold check of the same
/// sources does. The warm run goes first, so the cache it reads is the one the last step left.
#[track_caller]
pub fn warm_agrees(dir: &Path, what: &str) -> Value {
    let warm = check_answer(dir);
    let cold = cold_answer(dir);
    assert_eq!(
        warm, cold,
        "{what}: a check seeded from the cache answered differently from one that started cold"
    );
    warm
}

/// How many definitions a `ply check` at `dir` took from the cache, and how many it checked.
#[track_caller]
pub fn seeding(dir: &Path) -> (u64, u64) {
    let answer = json_of(&ply(dir).args(["check", "--json"]).output().unwrap());
    let definitions = &answer["front_end"]["definitions"];
    (
        definitions["seeded"].as_u64().expect("a seeded count"),
        definitions["checked"].as_u64().expect("a checked count"),
    )
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
    lock: Option<File>,
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
        Reservation {
            port,
            lock: Some(lock),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The server has answered on the port, so the port is the server's and the lock is spent.
    fn release(&mut self) {
        self.lock = None;
    }
}

/// Beside the test binary, so every test process in a run shares one lock whatever its `TMPDIR`.
fn lock_file() -> PathBuf {
    std::env::current_exe()
        .expect("the test binary's own path")
        .parent()
        .expect("the test binary lives in a directory")
        .join(".ply-ports.reserve")
}

/// Connects to the port `reserved` names, retrying until `ready` takes a connection or the child
/// exits. Taking the reservation is what ties readiness to the lock: it cannot be reached without
/// one, and a successful answer releases it, because the port is the server's from then on.
/// `ready` is the answer's proof that it is *this* server's -- `|_| true` once the reservation has
/// made the port ours alone.
pub fn connect_when_ready(
    reserved: &mut Reservation,
    child: &mut Child,
    deadline: Duration,
    mut ready: impl FnMut(&mut TcpStream) -> bool,
) -> Result<TcpStream, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], reserved.port()));
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
                reserved.release();
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
