use crate::harness::{fixtures, golden, port, records, repo_root};
use std::path::{Path, PathBuf};

/// The lexer's own fixtures, which are not the parser's.
fn edge_cases() -> PathBuf {
    fixtures().join("lexer")
}

fn dump(bytes: &[u8]) -> String {
    floats_to_bits(&port::dump("lexer.dump", bytes))
}

/// Rewrites every float record's decimal text to its bit pattern, the form the goldens hold.
fn floats_to_bits(dump: &str) -> String {
    let mut out = String::new();
    for record in dump.split_terminator(';') {
        let parts: Vec<&str> = record.splitn(4, ':').collect();
        match parts[..] {
            [start, end, "f", text] => match text.parse::<f64>() {
                Ok(v) => out.push_str(&format!("{start}:{end}:f:{:016x}", v.to_bits())),
                Err(_) => out.push_str(record),
            },
            _ => out.push_str(record),
        }
        out.push(';');
    }
    out
}

fn first_difference(golden: &str, actual: &str) -> Option<String> {
    let want = records(golden);
    let got = records(actual);
    for i in 0..want.len().max(got.len()) {
        let a = want.get(i).copied();
        let b = got.get(i).copied();
        if a != b {
            let from = i.saturating_sub(3);
            let mut report = format!("record {i} of {} differs\n", want.len());
            report.push_str(&format!(
                "  golden: {a:?}\n  port  : {b:?}\n  context (golden):\n"
            ));
            for (j, r) in want.iter().enumerate().skip(from).take(8) {
                report.push_str(&format!("    {j:>6} {r}\n"));
            }
            report.push_str("  context (port):\n");
            for (j, r) in got.iter().enumerate().skip(from).take(8) {
                report.push_str(&format!("    {j:>6} {r}\n"));
            }
            return Some(report);
        }
    }
    None
}

fn check(path: &Path) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let actual = dump(&bytes);
    let name = path
        .strip_prefix(repo_root())
        .expect("an input under the repository")
        .display()
        .to_string();
    if let Err(report) = golden::check("lexer", &name, &actual, first_difference) {
        panic!("{report}");
    }
    // A comparison over an empty corpus passes, so what was compared is printed.
    println!(
        "  {name:>52}  {:>7} bytes  {:>6} records",
        bytes.len(),
        records(&actual).len()
    );
}

fn ply_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.expect("a directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "ply"))
        .collect();
    out.sort();
    assert!(!out.is_empty(), "{} holds no .ply files", dir.display());
    out
}

#[test]
fn the_lexer_matches_its_golden_on_every_example() {
    for path in ply_files(&repo_root().join("examples")) {
        check(&path);
    }
}

#[test]
fn the_lexer_matches_its_golden_on_the_kernel_benchmarks() {
    for path in ply_files(&repo_root().join("benches").join("kernel")) {
        check(&path);
    }
}

#[test]
fn the_lexer_matches_its_golden_on_the_shipped_standard_library() {
    for path in ply_files(&repo_root().join("crates").join("ply-std").join("ply")) {
        check(&path);
    }
}

/// Nothing in the corpus above reaches a malformed literal, an out-of-range number, or an
/// unterminated string.
#[test]
fn the_lexer_matches_its_golden_on_the_hand_written_edge_cases() {
    for path in ply_files(&edge_cases()) {
        check(&path);
    }
}

// --- Non-ASCII input ----------------------------------------------------------

/// An identifier is ASCII: a letter outside it is refused, not lexed.
#[test]
fn a_unicode_identifier_is_refused() {
    assert_eq!(
        dump("let \u{e9} = 1\n".as_bytes()),
        "0:3:k:let;7:8:p:eq;9:10:n:1;11:11:e;4:6:!:X0001;"
    );
}

#[test]
fn a_unicode_space_is_refused() {
    assert_eq!(
        dump("a\u{a0}b\n".as_bytes()),
        "0:1:i:a;3:4:i:b;5:5:e;1:3:!:X0001;"
    );
}

#[test]
fn a_unicode_symbol_is_refused_with_no_token() {
    for text in ["a \u{20ac} b\n", "a \u{2014} b\n"] {
        assert_eq!(dump(text.as_bytes()), "0:1:i:a;6:7:i:b;8:8:e;2:5:!:X0001;");
    }
}

#[test]
fn a_backslash_before_a_non_ascii_character_is_a_bad_escape() {
    assert_eq!(
        dump("\"a\\\u{e9}b\"\n".as_bytes()),
        "0:7:s:61c3a962;8:8:e;2:5:!:E0001;"
    );
}

#[test]
fn a_non_ascii_character_inside_a_byte_literal_is_refused() {
    assert_eq!(
        dump("b\"a\u{e9}b\"\n".as_bytes()),
        "0:7:b:6162;8:8:e;3:5:!:E0001;"
    );
}

#[test]
fn a_non_ascii_character_inside_a_string_is_its_contents() {
    assert_eq!(
        dump("\"caf\u{e9}\"\n".as_bytes()),
        "0:7:s:636166c3a9;8:8:e;"
    );
}

#[test]
fn a_non_ascii_character_inside_a_comment_is_trivia() {
    assert_eq!(dump("// caf\u{e9}\n1\n".as_bytes()), "9:10:n:1;11:11:e;");
}

// --- Whether the comparison can fail ------------------------------------------

/// A fixture that has all five things to break: tokens, a payload, spans, and diagnostics.
fn a_mutable_dump() -> String {
    let path = edge_cases().join("bytes.ply");
    let actual = dump(&std::fs::read(&path).expect("the fixture"));
    assert!(first_difference(&actual, &actual).is_none());
    actual
}

#[test]
fn the_comparison_notices_a_token_whose_kind_is_wrong() {
    let actual = a_mutable_dump();
    let mutated = actual.replacen(":n:7;", ":i:7;", 1);
    assert_ne!(mutated, actual, "the mutation did not apply");
    assert!(first_difference(&actual, &mutated).is_some());
}

#[test]
fn the_comparison_notices_a_token_whose_payload_is_wrong() {
    let actual = a_mutable_dump();
    let mutated = actual.replacen("b:47455420", "b:47455421", 1);
    assert_ne!(mutated, actual, "the mutation did not apply");
    assert!(first_difference(&actual, &mutated).is_some());
}

/// A lexer with every kind and payload right and every offset wrong would pass a span-blind
/// comparator.
#[test]
fn the_comparison_notices_a_token_whose_span_is_wrong() {
    let actual = a_mutable_dump();
    let mutated = actual.replacen("4:11:b:", "4:12:b:", 1);
    assert_ne!(mutated, actual, "the mutation did not apply");
    assert!(first_difference(&actual, &mutated).is_some());
}

/// A zip that stops at the shorter of the two would swallow this.
#[test]
fn the_comparison_notices_a_token_that_is_missing_from_the_end() {
    let actual = a_mutable_dump();
    let kept = records(&actual);
    let mutated: String = kept[..kept.len() - 1]
        .iter()
        .map(|r| format!("{r};"))
        .collect();
    assert!(first_difference(&actual, &mutated).is_some());
}

/// The lexer answers with diagnostics beside the tokens, so a comparison of tokens alone passes a
/// lexer that silently accepts malformed input.
#[test]
fn the_comparison_notices_a_diagnostic_that_was_not_raised() {
    let actual = a_mutable_dump();
    let mutated = actual.replacen("42:44:!:E0001;", "", 1);
    assert_ne!(mutated, actual, "the mutation did not apply");
    assert!(first_difference(&actual, &mutated).is_some());
}
