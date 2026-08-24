//! `plydump <file.ply>` — the reference token dump, for reading by eye.
//!
//! The comparison itself is `tests/agreement.rs`; this exists so that a
//! disagreement can be looked at without a test harness in the way.

use std::io::Read;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: plydump <file.ply>   (or `-` for stdin)");
        std::process::exit(2);
    };
    let text = if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .expect("stdin is UTF-8");
        buf
    } else {
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    };
    for record in ply_lexer_spike_harness::records(&ply_lexer_spike_harness::reference_dump(&text))
    {
        println!("{record}");
    }
}
