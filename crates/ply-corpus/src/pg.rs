//! The floor and the lock: what the database benches need from libpq rather than Ply.
//!
//! The floor is the baseline a rung is compared against, and it has to be *outside* the language it
//! measures — a Rust or Ply client would be the harness measuring itself — so it is a small C
//! program, built from `benches/pg-floor/pg.c` on first use and cached beside it. The lock is a
//! second session that holds a row lock while a desk tries to write the row it holds, which is the
//! one thing a test needs a session to do *across* other work.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The tool's source and its build, both where the C it is made of lives.
fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benches/pg-floor/pg.c")
}

fn binary() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benches/pg-floor/build/pg")
}

/// The tool, built if it is missing or older than its source.
///
/// `pg_config` is asked where libpq's header and library are rather than guessing at them: a Homebrew
/// postgres lives outside the compiler's default search path, and a bench that silently compiled
/// against a different libpq than the server would measure the wrong library.
fn tool() -> Result<PathBuf> {
    let source = source();
    let binary = binary();
    let stale = match (std::fs::metadata(&source), std::fs::metadata(&binary)) {
        (Ok(src), Ok(built)) => built.modified()? < src.modified()?,
        _ => true,
    };
    if !stale {
        return Ok(binary);
    }
    if !source.is_file() {
        bail!("`{}` is missing", source.display());
    }
    let include = pg_config("--includedir")?;
    let libdir = pg_config("--libdir")?;
    if let Some(parent) = binary.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("making room for `{}`", binary.display()))?;
    }
    let out = Command::new("cc")
        .arg("-O2")
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .arg(format!("-I{include}"))
        .arg(format!("-L{libdir}"))
        .arg("-lpq")
        .output()
        .context("running `cc`; the database benches need a C compiler and libpq")?;
    if !out.status.success() {
        bail!(
            "compiling `{}`:\n{}",
            source.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(binary)
}

fn pg_config(flag: &str) -> Result<String> {
    let out = Command::new("pg_config")
        .arg(flag)
        .output()
        .with_context(|| format!("running `pg_config {flag}`; the database benches need libpq"))?;
    if !out.status.success() {
        bail!("`pg_config {flag}` exited {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The same statements as a rung runs, prepared once per connection, with no Ply in the path.
///
/// `connections` connections run `per` statements each at once, starting at `base` plus their own
/// share, exactly as the rung's tasks do; the answer is the wall clock around those statements,
/// which is what the rung measures.
pub fn floor(url: &str, workload: &str, connections: u32, per: u32, base: i64) -> Result<Duration> {
    let tool = tool()?;
    let out = Command::new(&tool)
        .args(["floor", url, workload])
        .args([connections.to_string(), per.to_string(), base.to_string()])
        .output()
        .with_context(|| format!("running `{}`", tool.display()))?;
    if !out.status.success() {
        bail!(
            "the floor exited {}:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let micros: u64 = text
        .split_whitespace()
        .last()
        .and_then(|n| n.parse().ok())
        .with_context(|| format!("the floor printed `{text}`, not a duration"))?;
    Ok(Duration::from_micros(micros))
}

/// Statements the language's own client refuses: the fixture's `drop`, `create` and `truncate`.
///
/// `std.db` accepts only DML from a program, which is the right rule for a program and the wrong one
/// for a harness building the thing it is about to measure. The statements are still the Ply
/// program's own `ddl`; only the execution is here.
pub fn sql(url: &str, statements: &str) -> Result<()> {
    let tool = tool()?;
    let out = Command::new(&tool)
        .args(["sql", url, statements])
        .output()
        .with_context(|| format!("running `{} sql`", tool.display()))?;
    if !out.status.success() {
        bail!(
            "`sql` exited {}:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// A session holding a row lock until it is released, dropped, or has its pipe closed.
pub struct Lock {
    child: Child,
}

impl Lock {
    /// Begins a transaction and runs `sql`, which must be a `select ... for update`.
    pub fn hold(url: &str, sql: &str) -> Result<Lock> {
        let tool = tool()?;
        let mut child = Command::new(&tool)
            .args(["lock", url, sql])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("running `{} lock`", tool.display()))?;
        // The tool prints `locked` once the row is held, so a test that goes on to need the lock
        // never races it.
        let ready = child
            .stdout
            .as_mut()
            .map(|out| {
                use std::io::Read;
                let mut line = [0u8; 7];
                out.read_exact(&mut line).map(|()| line)
            })
            .transpose();
        match ready {
            Ok(Some(line)) if &line == b"locked\n" => Ok(Lock { child }),
            _ => {
                let out = child.wait_with_output()?;
                bail!(
                    "the locking session could not take the lock:\n{}",
                    String::from_utf8_lossy(&out.stderr)
                )
            }
        }
    }

    /// Rolls back and waits for the session to end.
    pub fn release(mut self) -> Result<()> {
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = stdin.write_all(b"\n");
        }
        let status = self.child.wait()?;
        if !status.success() {
            bail!("the locking session exited {status}");
        }
        Ok(())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Closing the pipe releases the lock; a `wait` would block on a session that is already gone.
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
