//! The comparison this spike exists for: for every `.ply` file in the tree, the Ply lexer's token
//! stream against `crates/ply-syntax`'s.
//!
//! The lexer under test is the front end's own, `crates/ply-compiler/ply/lexer.ply`, entered
//! in-process through `port` as the bundle carries it; `PLY_C_EMITTER=ply:<dir>` enters a working
//! copy `stage` has bootstrapped. There used to be a copy of the lexer beside this file, and the
//! one not in the front end is the one that went stale.

use ply_compiler_diff::golden;
use ply_compiler_diff::port;
use ply_compiler_diff::tokens::{floats_to_bits, records, reference_dump};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits at <root>/crates/ply-compiler-diff")
        .to_path_buf()
}

/// The lexer differential's own fixtures, which are not the parser's.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("lexer-fixtures")
}

/// Run the Ply lexer over `bytes` and return its dump.
fn ply_dump(bytes: &[u8]) -> String {
    port::dump("lexer.dump", bytes)
}

/// The first record the two dumps disagree on, with context, or `None`.
fn first_difference(reference: &str, actual: &str) -> Option<String> {
    let want = records(reference);
    let got = records(actual);
    for i in 0..want.len().max(got.len()) {
        let a = want.get(i).copied();
        let b = got.get(i).copied();
        if a != b {
            let from = i.saturating_sub(3);
            let mut report = format!("record {i} of {} differs\n", want.len());
            report.push_str(&format!(
                "  rust: {a:?}\n  ply : {b:?}\n  context (rust):\n"
            ));
            for (j, r) in want.iter().enumerate().skip(from).take(8) {
                report.push_str(&format!("    {j:>6} {r}\n"));
            }
            report.push_str("  context (ply):\n");
            for (j, r) in got.iter().enumerate().skip(from).take(8) {
                report.push_str(&format!("    {j:>6} {r}\n"));
            }
            return Some(report);
        }
    }
    None
}

fn check_agreement(path: &Path) {
    let began = std::time::Instant::now();
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let text = String::from_utf8(bytes.clone())
        .unwrap_or_else(|e| panic!("{} is not UTF-8: {e}", path.display()));
    let reference = reference_dump(&text);
    let actual = floats_to_bits(&ply_dump(&bytes));
    let name = path
        .strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string();
    if let Err(report) = golden::check("lexer", &name, &reference, &actual, first_difference) {
        panic!("{report}");
    }
    // Printed under `--nocapture` so that a file the loop silently skipped is visible: a comparison
    // over an empty corpus passes.
    println!(
        "  agreed {:>52}  {:>7} bytes  {:>6} records  {:>7.2}s",
        path.display()
            .to_string()
            .rsplit('/')
            .take(2)
            .collect::<Vec<_>>()
            .join("/"),
        bytes.len(),
        records(&reference).len(),
        began.elapsed().as_secs_f64()
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
fn the_ply_lexer_agrees_with_the_rust_one_on_every_example() {
    for path in ply_files(&repo_root().join("examples")) {
        check_agreement(&path);
    }
}

#[test]
fn the_ply_lexer_agrees_with_the_rust_one_on_the_kernel_benchmarks() {
    for path in ply_files(&repo_root().join("benches").join("kernel")) {
        check_agreement(&path);
    }
}

#[test]
fn the_ply_lexer_agrees_with_the_rust_one_on_the_shipped_standard_library() {
    for path in ply_files(&repo_root().join("crates").join("ply-std").join("ply")) {
        check_agreement(&path);
    }
}

/// Nothing in the corpus above reaches a malformed literal, an out-of-range number, or an
/// unterminated string, so agreeing on it says nothing at all about the error paths, which are half
/// of what `lexer.rs` does.
#[test]
fn the_ply_lexer_agrees_with_the_rust_one_on_the_hand_written_edge_cases() {
    for path in ply_files(&fixtures()) {
        check_agreement(&path);
    }
}

/// Where the corpus's non-ASCII bytes are, which is the whole reason the four tests above can agree
/// at all.
#[test]
fn every_non_ascii_byte_in_the_corpus_is_somewhere_both_lexers_agree() {
    let mut non_ascii = 0usize;
    let mut in_string = 0usize;
    let mut files = 0usize;
    for dir in corpus_dirs() {
        for path in ply_files(&dir) {
            let bytes = std::fs::read(&path).expect("readable");
            let text = String::from_utf8(bytes.clone()).expect("UTF-8");
            let (tokens, _) = ply_syntax::lexer::lex(ply_span::SourceId(0), &text);
            files += 1;
            for (at, _) in bytes.iter().enumerate().filter(|(_, b)| **b >= 0x80) {
                non_ascii += 1;
                let covering = tokens
                    .iter()
                    .find(|t| (t.span.start as usize) <= at && at < (t.span.end as usize));
                match covering.map(|t| &t.kind) {
                    None => {}
                    Some(ply_syntax::lexer::TokenKind::Str(_)) => in_string += 1,
                    Some(other) => panic!(
                        "{} has a non-ASCII byte at offset {at} inside the token {other:?}. \
                         Every non-ASCII byte in this corpus is trivia or a string's \
                         contents, which is why the agreement tests pass over it; one \
                         anywhere else is a position where the Ply lexer answers \
                         differently on purpose, so the agreement claim has to be \
                         re-decided. README, Where this disagrees on purpose.",
                        path.display()
                    ),
                }
            }
        }
    }
    assert!(files >= 20, "only {files} files were checked");
    // Pinned so that a corpus that lost its prose — or gained a file this loop silently skipped —
    // is visible rather than quietly narrowing the claim.
    assert_eq!(
        (non_ascii, in_string),
        (301, 45),
        "the corpus holds {non_ascii} non-ASCII bytes of which {in_string} are inside string \
         literals, not the 1,543 and 45 this was written against"
    );
}

fn corpus_dirs() -> Vec<PathBuf> {
    vec![
        repo_root().join("examples"),
        repo_root().join("benches").join("kernel"),
        repo_root().join("crates").join("ply-std").join("ply"),
        fixtures(),
    ]
}

// --- Where the two lexers disagree on purpose --------------------------------

fn both_dumps(text: &str) -> (String, String) {
    (
        reference_dump(text),
        floats_to_bits(&ply_dump(text.as_bytes())),
    )
}

/// The one divergence that changes the **token stream** rather than only a diagnostic: `lexer.rs`
/// accepts any `char::is_alphabetic` as an identifier.
#[test]
fn the_two_lexers_differ_on_a_unicode_identifier() {
    let (rust, ply) = both_dumps("let \u{e9} = 1\n");
    assert_eq!(rust, "0:3:k:let;4:6:i:\u{e9};7:8:p:eq;9:10:n:1;11:11:e;");
    assert_eq!(ply, "0:3:k:let;7:8:p:eq;9:10:n:1;11:11:e;4:6:!:X0001;");
}

/// `char::is_whitespace` accepts sixteen characters outside ASCII.
#[test]
fn the_two_lexers_differ_on_a_unicode_space() {
    let (rust, ply) = both_dumps("a\u{a0}b\n");
    assert_eq!(rust, "0:1:i:a;3:4:i:b;5:5:e;");
    assert_eq!(ply, "0:1:i:a;3:4:i:b;5:5:e;1:3:!:X0001;");
}

/// The mildest of the three: both refuse, at the same span, with no token.
#[test]
fn the_two_lexers_differ_only_in_the_code_on_a_unicode_symbol() {
    for text in ["a \u{20ac} b\n", "a \u{2014} b\n"] {
        let (rust, ply) = both_dumps(text);
        assert_eq!(rust, "0:1:i:a;6:7:i:b;8:8:e;2:5:!:E0001;");
        assert_eq!(ply, "0:1:i:a;6:7:i:b;8:8:e;2:5:!:X0001;");
        assert_eq!(
            rust.replace("E0001", "X0001"),
            ply,
            "the two differ in more than the diagnostic code"
        );
    }
}

// The four shapes that look like they should diverge and do not.

#[test]
fn a_backslash_before_a_non_ascii_character_is_not_a_divergence() {
    let (rust, ply) = both_dumps("\"a\\\u{e9}b\"\n");
    assert_eq!(rust, "0:7:s:61c3a962;8:8:e;2:5:!:E0001;");
    assert_eq!(ply, rust);
}

#[test]
fn a_non_ascii_character_inside_a_byte_literal_is_not_a_divergence() {
    let (rust, ply) = both_dumps("b\"a\u{e9}b\"\n");
    assert_eq!(rust, "0:7:b:6162;8:8:e;3:5:!:E0001;");
    assert_eq!(ply, rust);
}

#[test]
fn a_non_ascii_character_inside_a_string_is_not_a_divergence() {
    let (rust, ply) = both_dumps("\"caf\u{e9}\"\n");
    assert_eq!(rust, "0:7:s:636166c3a9;8:8:e;");
    assert_eq!(ply, rust);
}

#[test]
fn a_non_ascii_character_inside_a_comment_is_not_a_divergence() {
    let (rust, ply) = both_dumps("// caf\u{e9}\n1\n");
    assert_eq!(rust, "9:10:n:1;11:11:e;");
    assert_eq!(ply, rust);
}

// --- Whether the comparison can fail ----------------------------------------

/// A fixture that has all five things to break: tokens, a payload, spans, and diagnostics.
fn a_mutable_dump() -> (String, String) {
    let path = fixtures().join("bytes.ply");
    let bytes = std::fs::read(&path).expect("the fixture");
    let text = String::from_utf8(bytes.clone()).expect("ASCII");
    let reference = reference_dump(&text);
    let actual = floats_to_bits(&ply_dump(&bytes));
    assert!(
        first_difference(&reference, &actual).is_none(),
        "the fixture must agree before it can be mutated"
    );
    (reference, actual)
}

#[test]
fn the_comparison_notices_a_token_whose_kind_is_wrong() {
    let (reference, actual) = a_mutable_dump();
    let mutated = actual.replacen(":n:7;", ":i:7;", 1);
    assert_ne!(mutated, actual, "the mutation did not apply");
    assert!(first_difference(&reference, &mutated).is_some());
}

#[test]
fn the_comparison_notices_a_token_whose_payload_is_wrong() {
    let (reference, actual) = a_mutable_dump();
    let mutated = actual.replacen("b:47455420", "b:47455421", 1);
    assert_ne!(mutated, actual, "the mutation did not apply");
    assert!(first_difference(&reference, &mutated).is_some());
}

/// The hazard worth naming: a lexer with every kind and payload right and every offset wrong would
/// agree with a span-blind comparator perfectly.
#[test]
fn the_comparison_notices_a_token_whose_span_is_wrong() {
    let (reference, actual) = a_mutable_dump();
    let mutated = actual.replacen("4:11:b:", "4:12:b:", 1);
    assert_ne!(mutated, actual, "the mutation did not apply");
    assert!(first_difference(&reference, &mutated).is_some());
}

/// A zip that stops at the shorter of the two would swallow this.
#[test]
fn the_comparison_notices_a_token_that_is_missing_from_the_end() {
    let (reference, actual) = a_mutable_dump();
    let records: Vec<&str> = records(&actual);
    let mutated: String = records[..records.len() - 1]
        .iter()
        .map(|r| format!("{r};"))
        .collect();
    assert!(first_difference(&reference, &mutated).is_some());
}

/// `lex` never fails — it answers with diagnostics *beside* the tokens — so a comparison of tokens
/// alone passes a lexer that silently accepts malformed input.
#[test]
fn the_comparison_notices_a_diagnostic_that_was_not_raised() {
    let (reference, actual) = a_mutable_dump();
    let mutated = actual.replacen("42:44:!:E0001;", "", 1);
    assert_ne!(mutated, actual, "the mutation did not apply");
    assert!(first_difference(&reference, &mutated).is_some());
}
