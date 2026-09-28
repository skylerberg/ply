//! Writes the corpus's own bench services out of the desk example, once.
//!
//! The request-path ladders used to build the program they measured by string surgery on
//! `examples/desk.ply`: splitting it at its tests, widening ten rows with `task.write`, swapping the
//! entry row, and replacing the call that serves the desk. The programs that produced are the
//! corpus's now, written down here by running the same intent directly rather than through four
//! textual passes — two of which had been broken since the example's entry point was refactored.
//!
//! Every variant is the example's own service with its *call* swapped, and the row left as the
//! example writes it: a row may say more than the body performs, which is what `run_memory` needs
//! when the host answers the atoms the entry does not.

use ply_corpus::w3::{Service, Variant};
use std::path::Path;

/// `source` with `main`'s *call* replaced by `call`, and the settings it reads left alone.
///
/// Every variant of the service is the example's own entry point with a different call: the desk is
/// served from the twin, from TLS, or from postgres, and where it listens is read from its settings
/// either way. The row is left as the example writes it — a row may say more than the body performs,
/// which is exactly what the twin needs — so only the last line of the body changes.
fn swap_call(source: &str, call: &str, row_atoms: &str) -> anyhow::Result<String> {
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
    // The row widens when the call performs what the entry point's own row does not name: a call
    // that reaches the database through no handler reaches `main` instead.
    let mut head = source[..end].to_string();
    if !row_atoms.is_empty()
        && let Some(at) = head.rfind("/ {")
    {
        head.insert_str(at + 3, row_atoms);
    }
    let mut out = String::with_capacity(source.len() + call.len());
    out.push_str(&head);
    out.push_str(&body[..last + 1]);
    out.push_str("  ");
    out.push_str(call);
    out.push_str(&after[close..]);
    Ok(out)
}

fn main() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = root.join("crates/ply-corpus/fixtures");
    std::fs::create_dir_all(&out)?;
    let service = Service::open(&root)?;
    let sequential = service.source(Variant::Sequential)?;
    let spawning = service.source(Variant::TaskPerConn)?;

    // The entry points the ladders run. Each is the call the harness used to splice in, written
    // against the port and connections the example reads from its own settings, so a run can point
    // at any port without rewriting the program.
    let writes = [
        ("desk-sequential-http.ply", &sequential, "run_memory(port, None, count)"),
        (
            "desk-sequential-https.ply",
            &sequential,
            "run_memory_tls(port, \"desk\", None, count)",
        ),
        ("desk-task-per-conn-http.ply", &spawning, "run_memory(port, None, count)"),
        (
            "desk-task-per-conn-https.ply",
            &spawning,
            "run_memory_tls(port, \"desk\", None, count)",
        ),
        ("desk-memory.ply", &sequential, "run_memory(port, key, count)"),
        (
            "desk-tls.ply",
            &sequential,
            "run_tls(port, \"desk\", count)",
        ),
        ("desk-postgres.ply", &sequential, ""),
    ];
    const DATABASE: &str = "db.query[items], db.execute[items], db.query[orders], \
         db.execute[orders], db.returning[orders], db.begin, db.commit, db.abort, db.rollback, ";
    for (name, source, body) in writes {
        let text = if body.is_empty() {
            source.to_string()
        } else {
            let atoms = if name == "desk-tls.ply" { DATABASE } else { "" };
            swap_call(source, body, atoms)?
        };
        let path = out.join(name);
        std::fs::write(&path, text)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
