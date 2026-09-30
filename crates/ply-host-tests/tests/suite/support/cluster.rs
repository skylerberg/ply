use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TOOLS: [&str; 3] = ["initdb", "postgres", "psql"];

/// The directory holding postgres and its tools: the first on PATH, else Homebrew's, else the
/// newest of Debian's per-version directories, which are off PATH.
fn bin_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    candidates.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    let mut debian: Vec<(u32, PathBuf)> = std::fs::read_dir("/usr/lib/postgresql")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let major = entry.file_name().to_str()?.parse().ok()?;
            Some((major, entry.path().join("bin")))
        })
        .collect();
    debian.sort();
    candidates.extend(debian.into_iter().rev().map(|(_, dir)| dir));
    candidates
        .into_iter()
        .find(|dir| TOOLS.iter().all(|tool| dir.join(tool).is_file()))
}

fn binary(name: &str) -> PathBuf {
    bin_dir().expect("postgres is installed").join(name)
}

/// Whether this machine can run a cluster. CI promises one, so there a missing one fails the test
/// that asked rather than skipping it.
pub fn available() -> bool {
    let found = bin_dir().is_some();
    assert!(
        found || std::env::var_os("CI").is_none(),
        "CI runs the postgres-backed tests, and this runner has no initdb, postgres and psql"
    );
    found
}

/// Stopped on drop, including during a panic unwind, so a failing test leaks no postgres.
pub struct Cluster {
    directory: tempfile::TempDir,
    server: Child,
    port: u16,
    pub database: String,
}

impl Cluster {
    /// A cluster whose TCP connections have to prove a password, which is what SCRAM is for.
    /// The local socket stays trust, so the harness can set the password up.
    pub fn start_with_password(database: &str, password: &str) -> Cluster {
        Cluster::launch(
            database,
            &["--auth-local=trust", "--auth-host=scram-sha-256"],
            Some(password),
        )
    }

    fn launch(database: &str, auth: &[&str], password: Option<&str>) -> Cluster {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let data = directory.path().join("data");
        let initdb = binary("initdb");

        let status = Command::new(&initdb)
            .args([
                "-D",
                data.to_str().expect("a utf-8 path"),
                "-U",
                "ply",
                "--no-sync",
                "-E",
                "UTF8",
                "--locale=C",
            ])
            .args(auth)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .expect("initdb runs");
        assert!(
            status.status.success(),
            "initdb failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );

        let port = free_port();
        let postgres = binary("postgres");
        // Run directly, not via `pg_ctl`, so the server is our child and dies with the harness.
        let server = Command::new(&postgres)
            .args([
                "-D",
                data.to_str().expect("a utf-8 path"),
                "-p",
                &port.to_string(),
                "-k",
                directory.path().to_str().expect("a utf-8 path"),
                "-c",
                "listen_addresses=127.0.0.1",
                "-c",
                "fsync=off",
                "-c",
                "full_page_writes=off",
                "-c",
                "synchronous_commit=off",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("postgres starts");

        let mut cluster = Cluster {
            directory,
            server,
            port,
            database: "postgres".to_string(),
        };
        cluster.wait_until_ready();
        if let Some(secret) = password {
            cluster.psql("postgres", &format!("alter role ply password '{secret}'"));
        }
        cluster.psql("postgres", &format!("create database {database}"));
        cluster.database = database.to_string();
        cluster
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn psql(&self, database: &str, sql: &str) -> String {
        let psql = binary("psql");
        let out = Command::new(&psql)
            .args([
                "-h",
                self.directory.path().to_str().expect("a utf-8 path"),
                "-p",
                &self.port.to_string(),
                "-U",
                "ply",
                "-d",
                database,
                "-v",
                "ON_ERROR_STOP=1",
                "-t",
                "-A",
                "-c",
                sql,
            ])
            .output()
            .expect("psql runs");
        assert!(
            out.status.success(),
            "`{sql}` failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let psql = binary("psql");
        while Instant::now() < deadline {
            let out = Command::new(&psql)
                .args([
                    "-h",
                    self.directory.path().to_str().expect("a utf-8 path"),
                    "-p",
                    &self.port.to_string(),
                    "-U",
                    "ply",
                    "-d",
                    "postgres",
                    "-c",
                    "select 1",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if out.is_ok_and(|s| s.success()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("postgres did not become ready within thirty seconds");
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        // Not `kill`: SIGKILL stops the postmaster signalling backends and freeing shared memory.
        let pg_ctl = binary("pg_ctl");
        if pg_ctl.is_file() {
            let stopped = Command::new(pg_ctl)
                .args([
                    "-D",
                    self.directory.path().join("data").to_str().unwrap_or(""),
                    "-m",
                    "immediate",
                    "-w",
                    "-t",
                    "20",
                    "stop",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if stopped.is_ok_and(|s| s.success()) {
                let _ = self.server.wait();
                return;
            }
        }
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

/// A port nothing is listening on, at the moment it is asked for.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("an address").port();
    drop(listener);
    port
}
