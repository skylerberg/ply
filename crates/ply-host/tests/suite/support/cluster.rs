//! A postgres cluster this test binary owns, start to finish.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Where the postgres binaries are, if this machine has them.
fn binary(name: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for prefix in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/lib/postgresql"] {
        let candidate = Path::new(prefix).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn available() -> bool {
    binary("initdb").is_some() && binary("postgres").is_some()
}

/// A running server, stopped when this is dropped — including while a panic unwinds, which is the
/// case that would otherwise leak a postgres process per failing test.
pub struct Cluster {
    directory: tempfile::TempDir,
    server: Child,
    port: u16,
    pub database: String,
}

impl Cluster {
    /// `initdb`, start, and create the database.
    pub fn start(database: &str) -> Cluster {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let data = directory.path().join("data");
        let initdb = binary("initdb").expect("initdb");

        let status = Command::new(&initdb)
            .args([
                "-D",
                data.to_str().expect("a utf-8 path"),
                "-U",
                "ply",
                "--auth=trust",
                "--no-sync",
                "-E",
                "UTF8",
                "--locale=C",
            ])
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
        let postgres = binary("postgres").expect("postgres");
        // Run the server directly rather than through `pg_ctl`, so it is this process's child and
        // dies with the harness rather than being daemonised past it.
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
        cluster.psql("postgres", &format!("create database {database}"));
        cluster.database = database.to_string();
        cluster
    }

    /// The `--db` string a run would be configured with.
    pub fn url(&self) -> String {
        self.url_for(&self.database)
    }

    pub fn url_for(&self, database: &str) -> String {
        format!(
            "postgresql://ply@127.0.0.1:{}/{database}?sslmode=disable&application_name=ply",
            self.port
        )
    }

    /// Run SQL out of band, through `psql`.
    pub fn psql(&self, database: &str, sql: &str) -> String {
        let psql = binary("psql").expect("psql");
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
        let psql = binary("psql").expect("psql");
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
        // `pg_ctl stop` rather than `Child::kill`, and the difference is not tidiness.
        if let Some(pg_ctl) = binary("pg_ctl") {
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
