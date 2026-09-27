//! A dependency that lives in a git repository: fetched once per project, then read like any other
//! package root.
//!
//! The front end judges a git dependency by the *key* its fetch is named under — `pkg::git_key`'s
//! `git+<url>@<rev>` — so nothing about where a tree lands reaches the package judgments. What
//! lands has to be the revision the manifest asked for: a `rev` may be a branch or a tag, and a
//! moved branch is caught by the lockfile, whose digest of the fetched sources is checked before a
//! build writes anything.

use ply_span::{Diagnostic, codes};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The directory a key is fetched into: under the project's own cache, named by the key, so two
/// projects never share a tree and one key is one directory.
pub fn cache_dir(root: &Path, key: &str) -> PathBuf {
    let digest = blake3::hash(key.as_bytes()).to_hex();
    root.join(".ply-cache").join("git").join(digest.as_str())
}

/// The tree `key` names, fetching it when the cache does not hold it. `Err` is a git that failed,
/// reported as the dependency's own trouble rather than as this tool's.
pub fn fetch(root: &Path, key: &str) -> Result<PathBuf, Diagnostic> {
    let (url, rev) = split(key).ok_or_else(|| malformed(key))?;
    let dir = cache_dir(root, key);
    if dir.join(".git").exists() {
        // Already fetched: the revision is either checked out or the fetch left nothing.
        return reset(&dir, rev).map(|()| dir);
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| failed(key, format!("its cache directory: {e}")))?;
    }
    clone(url, &dir, key)?;
    reset(&dir, rev)?;
    Ok(dir)
}

/// The url and revision a key names, read from the last `@`: a url may hold one of its own
/// (`git@host:path`), and a revision never does.
fn split(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("git+")?;
    let at = rest.rfind('@')?;
    let (url, rev) = rest.split_at(at);
    let rev = rev.strip_prefix('@')?;
    if url.is_empty() || rev.is_empty() {
        None
    } else {
        Some((url, rev))
    }
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    command.args(args);
    let out = command
        .output()
        .map_err(|e| format!("`git` could not run: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let mut why = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if why.is_empty() {
        why = format!("`git {}` exited {}", args.join(" "), out.status);
    }
    Err(why)
}

fn clone(url: &str, dir: &Path, key: &str) -> Result<(), Diagnostic> {
    git(
        None,
        &[
            "clone",
            "--quiet",
            "--no-checkout",
            url,
            &dir.display().to_string(),
        ],
    )
    .map_err(|why| failed(key, why))
}

fn reset(dir: &Path, rev: &str) -> Result<(), Diagnostic> {
    let key = dir.display().to_string();
    // A tree the cache already holds is used as it is. Fetching again would ask the remote what a
    // branch means *today*, and a load that already has the sources should not need the network —
    // which is also why a cleared cache is what picks up a moved branch, and why the lockfile's
    // digest is what catches it when that happens.
    if let Some(commit) = resolve(dir, rev) {
        return git(Some(dir), &["checkout", "--quiet", "--force", &commit])
            .map_err(|why| failed(&key, why));
    }
    // `--all --tags`: a revision may be any reachable commit, including one on a branch the clone
    // did not take.
    git(Some(dir), &["fetch", "--quiet", "--all", "--tags"]).map_err(|why| failed(&key, why))?;
    let commit = resolve(dir, rev).ok_or_else(|| {
        failed(
            &key,
            format!("`{rev}` names no revision: not a commit, tag or branch of this repository"),
        )
    })?;
    git(Some(dir), &["checkout", "--quiet", "--force", &commit]).map_err(|why| failed(&key, why))
}

/// The commit a revision names. A branch is looked up as the local ref a checkout would make or as
/// the remote-tracking one a clone makes (`HEAD` has no local branch yet), and a commit or tag
/// resolves as itself.
fn resolve(dir: &Path, rev: &str) -> Option<String> {
    for name in [rev.to_string(), format!("origin/{rev}")] {
        if let Ok(commit) = checked(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{name}^{{commit}}"),
            ],
        ) {
            return Some(commit);
        }
    }
    None
}

fn checked(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("`git` could not run: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn malformed(key: &str) -> Diagnostic {
    Diagnostic::error(
        codes::DEPENDENCY_UNUSABLE,
        format!("`{key}` is not a git key this `ply` writes"),
    )
    .note("a git dependency's key is `git+<url>@<rev>`, and the front end is what writes it")
}

fn failed(key: &str, why: String) -> Diagnostic {
    Diagnostic::error(
        codes::DEPENDENCY_FETCH,
        format!("the dependency `{key}` could not be fetched: {why}"),
    )
    .note("the tree is fetched into this project's own `.ply-cache`, and is fetched once")
    .note("check that `git` is installed and that the url and revision are right")
}
