//! No shipped definition nests deeper than the emitter has ever been asked to walk.
//!
//! The emitter recurses over an expression, so depth is what costs it and not size: a long run of
//! small statements is free, one expression built from many nested ones is not. Past what the
//! thread's native stack holds, `emit.emit_roots` raises and the failure surfaces in the bootstrap
//! fixpoint, which is the most expensive place in CI to learn it.
//!
//! This is a source-level proxy for that walk: bracket nesting, plus a level for each `?`, which
//! expands to a `match` wrapping the rest of its block, and each `else if`, which is one more
//! nested `if`. It is not the real ceiling — that is a function of the emitter's own frame size
//! and the stack it runs on. It is the cheap statement that nothing has become unusually deeper
//! than everything that compiles today.

use ply_cli::shipped;

/// Just past the deepest definition the compiler, the standard library and the program hold.
/// Raising it asks the emitter to recurse further than it ever has; flatten the definition
/// instead, by naming a helper or by looking a tag up rather than comparing down a chain.
const DEEPEST: usize = 32;

/// The text with `//` comments and string literals removed, so brackets inside them do not count.
fn bare(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if chars[i] == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn depth(body: &str) -> usize {
    let text = bare(body);
    let (mut at, mut deepest) = (0usize, 0usize);
    for c in text.chars() {
        match c {
            '(' | '[' | '{' => {
                at += 1;
                deepest = deepest.max(at);
            }
            ')' | ']' | '}' => at = at.saturating_sub(1),
            _ => {}
        }
    }
    deepest + text.matches('?').count() + text.matches("else if").count()
}

/// A line that opens a top-level item, and what to report it as.
fn opens(line: &str) -> Option<String> {
    let rest = line.strip_prefix("pub ").unwrap_or(line);
    for word in ["fn", "type", "test"] {
        if let Some(tail) = rest.strip_prefix(word).and_then(|t| t.strip_prefix(' ')) {
            let name: String = tail
                .trim_start_matches('"')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ' ')
                .collect();
            return Some(format!("{word} {}", name.trim()));
        }
    }
    None
}

/// Every top-level item of one module, as (what it is, its text).
fn items(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let heads: Vec<usize> = (0..lines.len())
        .filter(|&i| opens(lines[i]).is_some())
        .collect();
    let mut out = Vec::new();
    for (n, &i) in heads.iter().enumerate() {
        let end = heads.get(n + 1).copied().unwrap_or(lines.len());
        let Some(name) = opens(lines[i]) else {
            continue;
        };
        out.push((name, lines[i..end].join("\n")));
    }
    out
}

#[test]
fn no_shipped_definition_nests_deeper_than_the_emitter_walks() {
    let program: Vec<(String, String)> = shipped::PROGRAM_SOURCES
        .iter()
        .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
        .collect();
    let mut deepest: Vec<(usize, String, String)> = Vec::new();
    for (module, text) in ply_machine::shelf::sources().iter().chain(&program) {
        for (name, body) in items(text) {
            deepest.push((depth(&body), module.clone(), name));
        }
    }
    deepest.sort();
    deepest.reverse();
    assert!(!deepest.is_empty(), "no sources were scanned");
    let over: Vec<&(usize, String, String)> =
        deepest.iter().filter(|(d, _, _)| *d > DEEPEST).collect();
    assert!(
        over.is_empty(),
        "these nest deeper than {DEEPEST}, which is as deep as the emitter has ever walked; \
         flatten them or the bootstrap fixpoint fails with `emit.emit_roots` raised: {over:?}"
    );
    // The bound is only worth keeping while something is near it.
    assert!(
        deepest[0].0 > DEEPEST / 2,
        "nothing comes near {DEEPEST} any more ({:?}); lower the bound so it still says something",
        &deepest[..3]
    );
}
