//! `refdump <file.ply>` — the reference dump for one file, on stdout.
//!
//! The differential lives in `tests/agreement.rs`; this exists so that a
//! disagreement can be inspected without a test harness in the way, and so the
//! arming script can diff one side without running the other.
//!
//!   refdump <file>             the dump
//!   refdump --nodes <file>     the node count
//!   refdump --tags <file>      the distinct node tags, one per line
//!   refdump --bundle <file>    every fixture in a bundle, dumps joined by `~`
//!   refdump --bundle-tags <f>  the distinct tags over a whole bundle
//!
//! > **Withdrawn 2026-08-30.** A sixth mode stood here: *"`refdump
//! > --unexpanded <f>`   the dump with `effect_set::expand` projected out"*.
//! > Every dump this binary prints is now unexpanded — `reference_dump` enters
//! > at `ply_syntax::parse_unexpanded` — so the flag would name the only
//! > behaviour there is. `../GAPS.md` §11R.D.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, path) = match args.as_slice() {
        [p] => ("dump", p.clone()),
        [flag, p] if flag.starts_with("--") => (&flag[2..], p.clone()),
        _ => {
            eprintln!("usage: refdump [--nodes|--tags] <file.ply>");
            std::process::exit(2);
        }
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(2);
    });
    if mode == "bundle" || mode == "bundle-tags" {
        let dumps: Vec<String> = ply_parser_spike_harness::bundle(&text)
            .iter()
            .map(|f| ply_parser_spike_harness::reference_dump(f))
            .collect();
        if mode == "bundle" {
            println!("{}", dumps.join("~"));
        } else {
            for t in ply_parser_spike_harness::tags(&dumps.join("")) {
                println!("{t}");
            }
        }
        return;
    }
    let dump = ply_parser_spike_harness::reference_dump(&text);
    match mode {
        "dump" => println!("{dump}"),
        "nodes" => println!("{}", ply_parser_spike_harness::node_count(&dump)),
        "tags" => {
            for t in ply_parser_spike_harness::tags(&dump) {
                println!("{t}");
            }
        }
        other => {
            eprintln!("unknown mode: --{other}");
            std::process::exit(2);
        }
    }
}
