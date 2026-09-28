//! Writes the corpus's own bench services out of the desk example, once.
//!
//! The request-path ladders used to build the program they measured by string surgery on
//! `examples/desk.ply`: splitting it at its tests, widening ten rows with `task.write`, and
//! swapping the entry point for `run_memory` or `run_tls`. Those passes lived in the harness and
//! ran at every measurement, and two of them had been broken since the example's entry point was
//! refactored. They live here now: the programs are the corpus's, written down once, and a
//! measurement copies one.
//!
//! The shape of the catalogue is the harnesses' own: which accept loop runs (a connection at a
//! time, or a task per connection) crossed with what serves the routes (the in-memory twin over
//! HTTP, the same over TLS, postgres, postgres over TLS, or the twin behind `run_memory`). Every
//! one of them reads its port and connection count from the service's own settings, so a run can
//! point at any port without rewriting the program — which the old splice could not: it wrote the
//! harness's port into the source.

use std::path::Path;

/// Where `examples/desk.ply` stops being the service and starts being its tests.
const TESTS_MARKER: &str = "// --- Tests: the business, which needs no handler at all";

/// The database atoms a call reaches when it goes through no handler: `run_tls` serves the routes
/// from postgres directly, so `main` has to say so while the example's own row does not.
const DATABASE: &str = "db.query[items], db.execute[items], db.query[orders], db.execute[orders], \
     db.returning[orders], db.begin, db.commit, db.abort, db.rollback, ";

/// The example's service, without its tests.
fn served(root: &Path) -> anyhow::Result<String> {
    let desk = std::fs::read_to_string(root.join("examples/desk.ply"))?;
    let cut = desk
        .find(TESTS_MARKER)
        .ok_or_else(|| anyhow::anyhow!("the example no longer contains the tests marker"))?;
    Ok(desk[..cut].to_string())
}

/// A task per connection: the accept loop spawns instead of serving inline, and every row that
/// performs what the loop performs says so.
fn task_per_connection(service: &str) -> anyhow::Result<String> {
    let source = replace(
        service,
        "serve_connection(c, l);",
        "let t = task.spawn(|| serve_connection(c, l));",
    )?;
    let source = replace(
        &source,
        "1 + serve(listener, l, count - 1)",
        "let rest = serve(listener, l, count - 1);\n      task.join(t);\n      1 + rest",
    )?;
    const WIDENED: [&str; 10] = [
        "serve",
        "listen_and_serve",
        "listen_and_serve_tls",
        "run",
        "run_tls",
        "run_memory",
        "run_memory_tls",
        "memory_serving",
        // The entry point's own function, which serves the desk from postgres and so performs
        // whatever the accept loop does.
        "postgres",
        "main",
    ];
    WIDENED
        .iter()
        .try_fold(source, |acc, name| widen_row(&acc, name, "task.write, "))
}

/// `source` with `main`'s *call* replaced by `call`, and the settings it reads left alone.
///
/// Every mode of the service is the example's own entry point with a different call: the routes are
/// served from the twin, from TLS, or from postgres, and where it listens is read from its settings
/// either way. The row is left as the example writes it — a row may say more than the body performs,
/// which is exactly what the twin needs — so only the last line of the body changes; `atoms` is for
/// the one call that reaches the database through no handler.
fn swap_call(source: &str, call: &str, atoms: &str) -> anyhow::Result<String> {
    let header = ply_corpus::w3::main_header(source)?;
    let end = source
        .find(header)
        .ok_or_else(|| anyhow::anyhow!("`main`'s header was not found"))?
        + header.len();
    let after = &source[end..];
    let close = after
        .find("\n}")
        .ok_or_else(|| anyhow::anyhow!("`main` has no closing brace at column zero"))?;
    let body = &after[..close];
    let last = body
        .rfind('\n')
        .ok_or_else(|| anyhow::anyhow!("`main`'s body is one line"))?;
    let mut head = source[..end].to_string();
    if !atoms.is_empty()
        && let Some(at) = head.rfind("/ {")
    {
        head.insert_str(at + 3, atoms);
    }
    let mut out = String::with_capacity(source.len() + call.len());
    out.push_str(&head);
    out.push_str(&body[..last + 1]);
    out.push_str("  ");
    out.push_str(call);
    out.push_str(&after[close..]);
    Ok(out)
}

fn replace(source: &str, from: &str, to: &str) -> anyhow::Result<String> {
    if !source.contains(from) {
        anyhow::bail!("the example's service no longer contains:\n{from}");
    }
    Ok(source.replace(from, to))
}

/// Adds `atoms` to the head of the effect row `name` declares.
fn widen_row(source: &str, name: &str, atoms: &str) -> anyhow::Result<String> {
    let declaration = ["\nfn ", "\npub fn "]
        .into_iter()
        .find_map(|keyword| {
            let wanted = format!("{keyword}{name}(");
            source.find(&wanted).map(|at| at + 1)
        })
        .ok_or_else(|| anyhow::anyhow!("the example no longer defines `{name}`"))?;
    let body = &source[declaration..];
    let end = body
        .find('=')
        .ok_or_else(|| anyhow::anyhow!("`{name}` has no body"))?;
    let row = body[..end]
        .find("/ {")
        .ok_or_else(|| anyhow::anyhow!("`{name}` declares no effect row"))?;
    let at = declaration + row + "/ {".len();
    let mut out = String::with_capacity(source.len() + atoms.len());
    out.push_str(&source[..at]);
    out.push_str(atoms);
    out.push_str(&source[at..]);
    Ok(out)
}

fn main() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = root.join("crates/ply-corpus/fixtures");
    std::fs::create_dir_all(&out)?;
    let sequential = served(&root)?;
    let spawning = task_per_connection(&sequential)?;

    // Which accept loop, and what serves the routes. The names are the harnesses' own: `http`,
    // `https` and `memory` are the twin's three entry points, `postgres` and `tls` postgres'.
    let variants = [("sequential", &sequential), ("task-per-conn", &spawning)];
    let modes: [(&str, &str, &str); 5] = [
        ("http", "run_memory(port, None, count)", ""),
        ("https", "run_memory_tls(port, \"desk\", None, count)", ""),
        ("memory", "run_memory(port, key, count)", ""),
        ("postgres", "", ""),
        ("tls", "run_tls(port, \"desk\", count)", DATABASE),
    ];
    for (variant, source) in variants {
        for (mode, call, atoms) in modes {
            let text = if call.is_empty() {
                source.to_string()
            } else {
                swap_call(source, call, atoms)?
            };
            let path = out.join(format!("desk-{variant}-{mode}.ply"));
            std::fs::write(&path, text)?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}
