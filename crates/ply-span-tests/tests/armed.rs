//! The gate for the defect `CONTRIBUTING.md` §"The shape it keeps taking: declared, registered,
//! raised nowhere" counts: a mechanism named everywhere a reader would look for it and constructed
//! nowhere.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Registered codes that no production source constructs, each with the reason it is allowed to
/// stay that way.
const UNARMED_CODES: &[(&str, &str)] = &[
    (
        "DB_SCHEMA_MISMATCH",
        "E0435. Reserved for a schema verification that was specified and never \
         built. NOT fixed and NOT blessed: the database design says \
         \"E0435 and E0438 are registered and reserved but never emitted\", \
         and CONTRIBUTING.md \
         s\"Do not state a guarantee you have not armed\" carries it as its \
         first live example. This row exists so the gap is asserted rather than \
         remembered; it is not a decision that the gap is acceptable.",
    ),
    (
        "DB_UNMODELLED_SIDE_EFFECT",
        "E0438. Same reservation, and the database design calls \
         it \"the more serious of the two absences\". NOT fixed. Both codes are \
         in crates/ply-eval/src/host.rs's RESERVED_CODES, so a handler cannot \
         answer with either — a real, armed restriction (is_reserved_code), and \
         not the same thing as the code being raised.",
    ),
];

/// Variants of a covered enum that no production source constructs.
const UNARMED_VARIANTS: &[(&str, &str)] = &[
    (
        "Severity::Note",
        "Three renderers and no producer: crates/ply-span/src/render.rs:73 maps \
         it to ariadne's ReportKind::Advice, crates/ply-eval/src/differential.rs:803 \
         to \"note\", crates/ply-cli/src/commands/common.rs:55 to a dim \"note\". \
         Nothing builds one. Severity also derives Deserialize, so a Note could \
         in principle arrive from a stored diagnostic rather than from a \
         constructor — nothing in the workspace writes one, and the gate cannot \
         see serde either way. Disposition not decided here.",
    ),
    (
        "AssertionKind::Bool",
        "CONTRIBUTING.md item 14: Eq is the only variant ever built, at \
         crates/ply-test/src/slice.rs:345. Disposition is a separate \
         workstream's; this row holds the finding open.",
    ),
    (
        "AssertionKind::Panic",
        "CONTRIBUTING.md item 14. See AssertionKind::Bool.",
    ),
    (
        "AssertionKind::Runtime",
        "CONTRIBUTING.md item 14. See AssertionKind::Bool.",
    ),
    (
        "AssertionKind::UnhandledEffect",
        "CONTRIBUTING.md item 14. See AssertionKind::Bool.",
    ),
    (
        "AssertionKind::RecursionLimit",
        "CONTRIBUTING.md item 14, and row 4 of the catalogue in \
         CONTRIBUTING.md s\"The shape it keeps taking\". \
         crates/ply-eval/src/limit.rs:82 quotes a withdrawn claim that the failure \
         0004's RecursionLimit \"still classifies it\" and records that nothing \
         does. See AssertionKind::Bool.",
    ),
    (
        "AssertionKind::Deadlock",
        "CONTRIBUTING.md item 14. See AssertionKind::Bool.",
    ),
    (
        "Event::Enter",
        "CONTRIBUTING.md item 15: nothing outside crates/ply-test-tests/tests/ \
         constructs a SliceBuilder, so nothing calls SliceBuilder::record, so no \
         Event is ever built. SliceBuilder::record does match on all three \
         variants — that is a consumer, not a producer.",
    ),
    (
        "Event::Return",
        "CONTRIBUTING.md item 15. See Event::Enter.",
    ),
    (
        "Event::Perform",
        "CONTRIBUTING.md item 15, and row 5 of the catalogue in \
         CONTRIBUTING.md s\"The shape it keeps taking\". See Event::Enter.",
    ),
];

/// Functions that take a code and hand it to `Diagnostic::error`/`warning` unchanged.
const CODE_INDIRECTION: &[Indirection] = &[Indirection {
    file: "crates/ply-syntax/src/lexer.rs",
    function: "error",
    reason: "Lexer::error(code, message, span, label) pushes \
             Diagnostic::error(code, message).primary(span, label) and does \
             nothing else with the code. Private to the lexer.",
}];

/// Covered enum names that more than one covered enum declares.
const AMBIGUOUS_ENUM_NAMES: &[(&str, &str)] = &[(
    "Reason",
    "crates/ply-test/src/lib.rs's Reason (why a test was selected) and \
     crates/ply-test/src/obligation.rs's Reason (where a discharge came from) \
     share the variant name New. Every variant of both is armed today, so the \
     ambiguity changes no answer; if one of them dies while the other keeps a \
     variant of the same name, this gate will not see it.",
)];

struct Indirection {
    file: &'static str,
    function: &'static str,
    /// Read by `no_allowlist_entry_has_outlived_its_reason`, so that an entry here cannot be added
    /// without one.
    reason: &'static str,
}

/// Every `pub enum` under these directories is covered by the variant half.
const COVERED_ENUM_ROOTS: &[&str] = &["crates/ply-test/src"];

/// Individually covered enums outside `COVERED_ENUM_ROOTS`, as `(file, name)`.
const COVERED_ENUMS: &[(&str, &str)] = &[("crates/ply-span/src/lib.rs", "Severity")];

/// Replaces the contents of comments, string literals, raw string literals and character literals
/// with spaces, preserving every byte offset and every newline.
fn blank_literals_and_comments(src: &[u8]) -> Vec<u8> {
    let mut out = src.to_vec();
    let n = src.len();
    let mut i = 0;
    while i < n {
        match src[i] {
            b'/' if i + 1 < n && src[i + 1] == b'/' => {
                let mut j = i;
                while j < n && src[j] != b'\n' {
                    j += 1;
                }
                blank(&mut out, i, j);
                i = j;
            }
            b'/' if i + 1 < n && src[i + 1] == b'*' => {
                let mut depth = 1usize;
                let mut j = i + 2;
                while j < n && depth > 0 {
                    if src[j] == b'/' && j + 1 < n && src[j + 1] == b'*' {
                        depth += 1;
                        j += 2;
                    } else if src[j] == b'*' && j + 1 < n && src[j + 1] == b'/' {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                blank(&mut out, i, j);
                i = j;
            }
            b'r' if i + 1 < n
                && (src[i + 1] == b'#' || src[i + 1] == b'"')
                && !(i > 0 && is_ident_byte(src[i - 1])) =>
            {
                let mut k = i + 1;
                let mut hashes = 0usize;
                while k < n && src[k] == b'#' {
                    hashes += 1;
                    k += 1;
                }
                if k < n && src[k] == b'"' {
                    let mut j = k + 1;
                    let mut end = n;
                    while j < n {
                        if src[j] == b'"' && (1..=hashes).all(|z| src.get(j + z) == Some(&b'#')) {
                            end = j + 1 + hashes;
                            break;
                        }
                        j += 1;
                    }
                    let end = end.min(n);
                    blank(&mut out, i, end);
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'"' => {
                let mut j = i + 1;
                while j < n {
                    if src[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if src[j] == b'"' {
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                let j = j.min(n);
                blank(&mut out, i, j);
                i = j;
            }
            b'\'' => match char_literal_end(src, i) {
                Some(end) => {
                    blank(&mut out, i, end);
                    i = end;
                }
                None => i += 1,
            },
            _ => i += 1,
        }
    }
    out
}

fn blank(out: &mut [u8], from: usize, to: usize) {
    let n = out.len();
    for b in &mut out[from.min(n)..to.min(n)] {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `Some(end)` for a character literal starting at `i`, `None` for a lifetime.
fn char_literal_end(src: &[u8], i: usize) -> Option<usize> {
    let n = src.len();
    if src.get(i + 1) == Some(&b'\\') {
        let mut j = i + 2;
        while j < n && j - i < 12 && src[j] != b'\'' && src[j] != b'\n' {
            j += 1;
        }
        return (src.get(j) == Some(&b'\'')).then_some(j + 1);
    }
    let lead = *src.get(i + 1)?;
    let width = match lead {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    (src.get(i + 1 + width) == Some(&b'\'')).then_some(i + 2 + width)
}

/// Blanks every `#[cfg(test)]` item.
fn blank_cfg_test_blocks(text: &[u8]) -> Vec<u8> {
    const ATTR: &[u8] = b"#[cfg(test)]";
    let mut out = text.to_vec();
    let mut i = 0;
    while i + ATTR.len() <= text.len() {
        if &text[i..i + ATTR.len()] != ATTR {
            i += 1;
            continue;
        }
        let mut j = i + ATTR.len();
        let mut delimiter = None;
        while j < text.len() {
            match text[j] {
                b'(' | b'[' => j = delim_close(text, j),
                b'{' | b';' => {
                    delimiter = Some(j);
                    break;
                }
                _ => j += 1,
            }
        }
        let Some(j) = delimiter else { break };
        if text[j] == b'{' {
            let end = delim_close(text, j);
            blank(&mut out, i, end);
            i = end;
        } else if header_declares_a_module(&text[i + ATTR.len()..j]) {
            i += ATTR.len();
        } else {
            // A `;`-terminated item that is not a module declaration — a `const`, a `static`, a
            // `type`, a `use`.
            blank(&mut out, i, j + 1);
            i = j + 1;
        }
    }
    out
}

/// Whether an item header between `#[cfg(test)]` and its `;` is a `mod` declaration.
fn header_declares_a_module(header: &[u8]) -> bool {
    let mut i = 0;
    while i < header.len() {
        if !(header[i].is_ascii_alphabetic() || header[i] == b'_') {
            i += 1;
            continue;
        }
        let (word, end) = ident_at(header, i);
        if word == b"mod" && (i == 0 || !is_ident_byte(header[i - 1])) {
            return true;
        }
        i = end;
    }
    false
}

/// `text[i]` is an opening delimiter; the index just past its match.
fn delim_close(text: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < text.len() {
        match text[j] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    text.len()
}

/// The matching opener for the closer at `i`, scanning backwards.
fn delim_open(text: &[u8], i: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = i;
    loop {
        match text[j] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
        if j == 0 {
            return None;
        }
        j -= 1;
    }
}

fn ident_at(text: &[u8], i: usize) -> (&[u8], usize) {
    let mut end = i;
    while end < text.len() && is_ident_byte(text[end]) {
        end += 1;
    }
    (&text[i..end], end)
}

fn skip_ws(text: &[u8], mut i: usize, hi: usize) -> usize {
    while i < hi && text[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn line_of(text: &[u8], offset: usize) -> usize {
    text[..offset.min(text.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

/// Byte ranges that are pattern positions or `use` paths — the two places a `Type::Variant` can
/// appear without anything being constructed.
fn pattern_and_use_regions(text: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    scan(text, 0, text.len(), &mut out);
    out
}

fn scan(text: &[u8], lo: usize, hi: usize, out: &mut Vec<(usize, usize)>) {
    let mut i = lo;
    while i < hi {
        let c = text[i];
        if !(c.is_ascii_alphabetic() || c == b'_') {
            i += 1;
            continue;
        }
        if i > lo && is_ident_byte(text[i - 1]) {
            let (_, end) = ident_at(text, i);
            i = end;
            continue;
        }
        let (word, end) = ident_at(text, i);
        match word {
            b"match" => {
                let mut j = end;
                while j < hi {
                    match text[j] {
                        b'(' | b'[' => j = delim_close(text, j),
                        b'{' | b';' => break,
                        _ => j += 1,
                    }
                }
                if j < hi && text[j] == b'{' {
                    let close = delim_close(text, j).min(hi);
                    scan_match(text, j, close, out);
                    i = close;
                } else {
                    i = end;
                }
            }
            // `if let`, `while let` and a plain `let` are the same shape: a pattern between the
            // keyword and the `=`.
            b"let" => {
                let mut j = end;
                while j < hi {
                    match text[j] {
                        b'(' | b'[' | b'{' => j = delim_close(text, j),
                        b';' => break,
                        b'=' if text.get(j + 1) != Some(&b'=')
                            && text.get(j + 1) != Some(&b'>') =>
                        {
                            break;
                        }
                        _ => j += 1,
                    }
                }
                out.push((end, j));
                i = j;
            }
            b"for" => {
                let mut j = end;
                while j < hi {
                    match text[j] {
                        b'(' | b'[' => j = delim_close(text, j),
                        b'{' | b';' => break,
                        c if c.is_ascii_alphabetic() || c == b'_' => {
                            let (w, e) = ident_at(text, j);
                            if w == b"in" {
                                break;
                            }
                            j = e;
                        }
                        _ => j += 1,
                    }
                }
                out.push((end, j));
                i = j;
            }
            b"use" => {
                let mut j = end;
                while j < hi && text[j] != b';' {
                    j += 1;
                }
                out.push((end, j));
                i = j;
            }
            b"matches" => {
                let k = skip_ws(text, end, hi);
                if text.get(k) == Some(&b'!') {
                    let k = skip_ws(text, k + 1, hi);
                    if matches!(text.get(k), Some(b'(') | Some(b'[')) {
                        let close = delim_close(text, k).min(hi);
                        let mut p = k + 1;
                        while p + 1 < close {
                            match text[p] {
                                b'(' | b'[' | b'{' => p = delim_close(text, p),
                                b',' => break,
                                _ => p += 1,
                            }
                        }
                        out.push((p, close.saturating_sub(1)));
                        i = close;
                        continue;
                    }
                }
                i = end;
            }
            _ => i = end,
        }
    }
}

#[derive(PartialEq)]
enum Arm {
    Head,
    Guard,
}

fn scan_match(text: &[u8], brace: usize, close: usize, out: &mut Vec<(usize, usize)>) {
    let end = close.saturating_sub(1);
    let mut i = brace + 1;
    let mut state = Arm::Head;
    let mut head_start = i;
    while i < end {
        let c = text[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match state {
            Arm::Head => {
                // A pattern's own groups — tuple, slice and struct patterns — are inside the head
                // range, so they need no separate marking.
                if matches!(c, b'(' | b'[' | b'{') {
                    i = delim_close(text, i).min(end);
                } else if c == b'=' && text.get(i + 1) == Some(&b'>') {
                    out.push((head_start, i));
                    i = consume_arm_body(text, i + 2, end, out);
                    head_start = i;
                } else if c.is_ascii_alphabetic() || c == b'_' {
                    let (w, e) = ident_at(text, i);
                    // A guard is an expression, not a pattern.
                    if w == b"if" {
                        out.push((head_start, i));
                        state = Arm::Guard;
                    }
                    i = e;
                } else {
                    i += 1;
                }
            }
            Arm::Guard => {
                if matches!(c, b'(' | b'[' | b'{') {
                    let j = delim_close(text, i).min(end);
                    scan(text, i + 1, j.saturating_sub(1), out);
                    i = j;
                } else if c == b'=' && text.get(i + 1) == Some(&b'>') {
                    i = consume_arm_body(text, i + 2, end, out);
                    state = Arm::Head;
                    head_start = i;
                } else if c.is_ascii_alphabetic() || c == b'_' {
                    let (_, e) = ident_at(text, i);
                    i = e;
                } else {
                    i += 1;
                }
            }
        }
    }
    if state == Arm::Head && head_start < end {
        out.push((head_start, end));
    }
}

/// A block body ends at its own `}`, with the comma after it optional; any other body ends at the
/// next comma outside a group.
fn consume_arm_body(text: &[u8], i: usize, end: usize, out: &mut Vec<(usize, usize)>) -> usize {
    let mut i = skip_ws(text, i, end);
    if text.get(i) == Some(&b'{') {
        let j = delim_close(text, i).min(end);
        scan(text, i + 1, j.saturating_sub(1), out);
        i = skip_ws(text, j, end);
        if text.get(i) == Some(&b',') {
            i += 1;
        }
        return i;
    }
    let start = i;
    while i < end {
        match text[i] {
            b'(' | b'[' | b'{' => i = delim_close(text, i).min(end),
            b',' => break,
            _ => i += 1,
        }
    }
    scan(text, start, i, out);
    if i < end { i + 1 } else { i }
}

struct Source {
    rel: String,
    text: Vec<u8>,
    masked: Vec<bool>,
}

impl Source {
    fn is_pattern_or_use(&self, at: usize) -> bool {
        self.masked.get(at).copied().unwrap_or(false)
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the workspace root is two directories above crates/ply-span")
}

/// Package names from `[workspace] members`, read out of the root manifest as text — the same
/// source `.github/ci-shards.sh` reads, for the same reason.
fn workspace_members(root: &Path) -> Vec<String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("a workspace manifest");
    let list = manifest
        .split_once("members = [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| list)
        .expect("[workspace] members is a bracketed list");
    list.lines()
        .filter_map(|line| line.trim().strip_prefix("\"crates/"))
        .filter_map(|line| line.split_once('"'))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Every `mod NAME;` in a file, with whether `#[cfg(test)]` sits on it.
fn mod_declarations(text: &[u8]) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if !(text[i].is_ascii_alphabetic() || text[i] == b'_') {
            i += 1;
            continue;
        }
        let (word, end) = ident_at(text, i);
        if word != b"mod" || (i > 0 && is_ident_byte(text[i - 1])) {
            i = end;
            continue;
        }
        let name_at = skip_ws(text, end, text.len());
        let (name, name_end) = ident_at(text, name_at);
        let after = skip_ws(text, name_end, text.len());
        if !name.is_empty() && text.get(after) == Some(&b';') {
            out.push((
                String::from_utf8_lossy(name).into_owned(),
                attributes_include_cfg_test(text, i),
            ));
        }
        i = end;
    }
    out
}

fn attributes_include_cfg_test(text: &[u8], mod_at: usize) -> bool {
    let mut p = mod_at;
    loop {
        while p > 0 && text[p - 1].is_ascii_whitespace() {
            p -= 1;
        }
        // `pub(crate)` and friends.
        if p > 0
            && text[p - 1] == b')'
            && let Some(open) = delim_open(text, p - 1)
        {
            let mut q = open;
            while q > 0 && text[q - 1].is_ascii_whitespace() {
                q -= 1;
            }
            if q >= 3 && &text[q - 3..q] == b"pub" {
                p = q - 3;
                continue;
            }
        }
        if p >= 3 && &text[p - 3..p] == b"pub" && (p == 3 || !is_ident_byte(text[p - 4])) {
            p -= 3;
            continue;
        }
        if p > 0
            && text[p - 1] == b']'
            && let Some(open) = delim_open(text, p - 1)
            && open > 0
            && text[open - 1] == b'#'
        {
            let attr: Vec<u8> = text[open + 1..p - 1]
                .iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            if attr == b"cfg(test)" {
                return true;
            }
            p = open - 1;
            continue;
        }
        return false;
    }
}

/// Walks `mod` declarations from every crate root and returns the production files: everything
/// reachable without passing through a `#[cfg(test)] mod`.
fn production_sources(root: &Path) -> Vec<Source> {
    let mut reached: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let mut queue: Vec<(PathBuf, bool)> = Vec::new();

    for member in workspace_members(root) {
        let src = root.join("crates").join(&member).join("src");
        for entry in ["lib.rs", "main.rs"] {
            let path = src.join(entry);
            if path.is_file() {
                queue.push((path, false));
            }
        }
        if let Ok(bins) = std::fs::read_dir(src.join("bin")) {
            for bin in bins.flatten() {
                if bin.path().extension().is_some_and(|e| e == "rs") {
                    queue.push((bin.path(), false));
                }
            }
        }
    }

    while let Some((path, test_only)) = queue.pop() {
        match reached.get(&path) {
            // A module reachable by both a production and a test path is production: it compiles
            // into the library.
            Some(&seen) if seen == test_only || !seen => continue,
            _ => {}
        }
        reached.insert(path.clone(), test_only);
        let raw = std::fs::read(&path).expect("a readable source file");
        assert!(
            !contains(&raw, b"#[path"),
            "{} uses #[path], which this resolver does not follow — teach it, or the \
             module it names silently drops out of the production set",
            path.display()
        );
        // Blanking recognises `#[cfg(test)]` exactly.
        for spelling in [b"cfg(all(test".as_slice(), b"cfg(any(test".as_slice()] {
            assert!(
                !contains(&blank_literals_and_comments(&raw), spelling),
                "{} uses `{}`, which blank_cfg_test_blocks does not recognise — teach it, \
                 or that item is scanned as production source",
                path.display(),
                String::from_utf8_lossy(spelling)
            );
        }
        let blanked = blank_literals_and_comments(&raw);
        let dir = path.parent().expect("a source file has a directory");
        let stem = path.file_stem().expect("a source file has a stem");
        let base = if matches!(stem.to_str(), Some("lib") | Some("main") | Some("mod")) {
            dir.to_path_buf()
        } else {
            dir.join(stem)
        };
        for (name, is_test) in mod_declarations(&blanked) {
            for candidate in [
                base.join(format!("{name}.rs")),
                base.join(&name).join("mod.rs"),
            ] {
                if candidate.is_file() {
                    queue.push((candidate, test_only || is_test));
                    break;
                }
            }
        }
    }

    let mut sources: Vec<Source> = reached
        .into_iter()
        .filter(|(_, test_only)| !test_only)
        .map(|(path, _)| {
            let raw = std::fs::read(&path).expect("a readable source file");
            let text = blank_cfg_test_blocks(&blank_literals_and_comments(&raw));
            let mut masked = vec![false; text.len()];
            for (from, to) in pattern_and_use_regions(&text) {
                for m in &mut masked[from.min(text.len())..to.min(text.len())] {
                    *m = true;
                }
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            Source { rel, text, masked }
        })
        .collect();
    sources.sort_by(|a, b| a.rel.cmp(&b.rel));
    sources
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    (0..=haystack.len() - needle.len())
        .filter(|&i| &haystack[i..i + needle.len()] == needle)
        .collect()
}

/// `NAME -> ("E0435", line)` for every `pub const` in `ply_span::codes`.
fn declared_codes(root: &Path) -> BTreeMap<String, (String, usize)> {
    let raw = std::fs::read(root.join("crates/ply-span/src/lib.rs")).expect("ply-span's lib.rs");
    let blanked = blank_literals_and_comments(&raw);
    let at = find_all(&blanked, b"pub mod codes")
        .into_iter()
        .next()
        .expect("ply-span declares `pub mod codes`");
    let open = blanked[at..]
        .iter()
        .position(|b| *b == b'{')
        .expect("the codes module has a body")
        + at;
    let close = delim_close(&blanked, open);

    let mut out = BTreeMap::new();
    for start in find_all(&blanked[open..close], b"pub const") {
        let start = open + start;
        let name_at = skip_ws(&blanked, start + b"pub const".len(), close);
        let (name, name_end) = ident_at(&blanked, name_at);
        // `skip_ws` over `blanked` would walk straight over the blanked string literal and land on
        // the `;`.
        let value_at = match blanked[name_end..close].iter().position(|b| *b == b'=') {
            Some(eq) => skip_ws(&raw, name_end + eq + 1, close),
            None => continue,
        };
        if raw.get(value_at) != Some(&b'"') {
            continue;
        }
        let end = raw[value_at + 1..close]
            .iter()
            .position(|b| *b == b'"')
            .map(|p| value_at + 1 + p)
            .expect("a terminated code literal");
        out.insert(
            String::from_utf8_lossy(name).into_owned(),
            (
                String::from_utf8_lossy(&raw[value_at + 1..end]).into_owned(),
                line_of(&raw, start),
            ),
        );
    }
    out
}

/// The `(name, codes::NAME, "E0001")` rows of the registry table in ply-span's own test module,
/// read as text — an integration test cannot call into a `#[cfg(test)]` module.
fn registry_rows(root: &Path) -> BTreeSet<String> {
    let raw = std::fs::read(root.join("crates/ply-span/src/lib.rs")).expect("ply-span's lib.rs");
    let blanked = blank_literals_and_comments(&raw);
    let at = find_all(&blanked, b"let registry = [")
        .into_iter()
        .next()
        .expect("the registry table is `let registry = [`");
    let open = at + b"let registry = ".len();
    let close = delim_close(&blanked, open);
    find_all(&blanked[open..close], b"codes::")
        .into_iter()
        .map(|p| {
            let (name, _) = ident_at(&blanked, open + p + b"codes::".len());
            String::from_utf8_lossy(name).into_owned()
        })
        .collect()
}

/// Every `Diagnostic::error`/`warning` call in production, as `(source index, offset, first
/// argument as written)`.
fn constructor_calls(sources: &[Source]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (idx, source) in sources.iter().enumerate() {
        for name in [b"Diagnostic::error".as_slice(), b"Diagnostic::warning"] {
            for at in find_all(&source.text, name) {
                let paren = skip_ws(&source.text, at + name.len(), source.text.len());
                if source.text.get(paren) != Some(&b'(') {
                    continue;
                }
                let arg_at = skip_ws(&source.text, paren + 1, source.text.len());
                let mut end = arg_at;
                while end < source.text.len()
                    && (is_ident_byte(source.text[end]) || source.text[end] == b':')
                {
                    end += 1;
                }
                out.push((
                    idx,
                    at,
                    String::from_utf8_lossy(&source.text[arg_at..end]).into_owned(),
                ));
            }
        }
    }
    out
}

fn code_from_path(arg: &str) -> Option<&str> {
    let (prefix, name) = arg.rsplit_once("::")?;
    (prefix == "codes" || prefix.ends_with("::codes")).then_some(name)
}

/// The `fn` a byte offset sits in: the nearest `fn NAME` before it.
fn enclosing_function(text: &[u8], at: usize) -> String {
    find_all(&text[..at], b"fn ")
        .into_iter()
        .rev()
        .find(|&p| p == 0 || !is_ident_byte(text[p - 1]))
        .map(|p| {
            let (name, _) = ident_at(text, skip_ws(text, p + 3, at));
            String::from_utf8_lossy(name).into_owned()
        })
        .unwrap_or_else(|| "<no enclosing fn>".to_string())
}

fn armed_codes(sources: &[Source]) -> BTreeSet<String> {
    let mut armed: BTreeSet<String> = constructor_calls(sources)
        .iter()
        .filter_map(|(_, _, arg)| code_from_path(arg).map(str::to_string))
        .collect();
    for wrapper in CODE_INDIRECTION {
        assert!(
            wrapper.reason.len() > 40,
            "CODE_INDIRECTION entry `{}::{}` has no real reason",
            wrapper.file,
            wrapper.function
        );
        let Some(source) = sources.iter().find(|s| s.rel == wrapper.file) else {
            continue;
        };
        let needle = format!(".{}(", wrapper.function).into_bytes();
        for at in find_all(&source.text, &needle) {
            let arg_at = skip_ws(&source.text, at + needle.len(), source.text.len());
            let mut end = arg_at;
            while end < source.text.len()
                && (is_ident_byte(source.text[end]) || source.text[end] == b':')
            {
                end += 1;
            }
            let arg = String::from_utf8_lossy(&source.text[arg_at..end]);
            if let Some(code) = code_from_path(&arg) {
                armed.insert(code.to_string());
            }
        }
    }
    armed
}

struct CoveredEnum {
    name: String,
    file: String,
    line: usize,
    variants: Vec<String>,
}

fn variant_names(body: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            b'#' if body.get(i + 1) == Some(&b'[') => i = delim_close(body, i + 1),
            b'(' | b'[' | b'{' => i = delim_close(body, i),
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let (name, end) = ident_at(body, i);
                out.push(String::from_utf8_lossy(name).into_owned());
                // A discriminant — `Foo = 1,` — is not a variant name.
                let after = skip_ws(body, end, body.len());
                if body.get(after) == Some(&b'=') {
                    i = after;
                    while i < body.len() && body[i] != b',' {
                        i += 1;
                    }
                } else {
                    i = end;
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn covered_enums(sources: &[Source]) -> Vec<CoveredEnum> {
    let mut out = Vec::new();
    for source in sources {
        let in_root = COVERED_ENUM_ROOTS
            .iter()
            .any(|root| source.rel.starts_with(root));
        let named: Vec<&str> = COVERED_ENUMS
            .iter()
            .filter(|(file, _)| *file == source.rel)
            .map(|(_, name)| *name)
            .collect();
        if !in_root && named.is_empty() {
            continue;
        }
        for at in find_all(&source.text, b"pub enum ") {
            let name_at = skip_ws(&source.text, at + b"pub enum ".len(), source.text.len());
            let (name, name_end) = ident_at(&source.text, name_at);
            let name = String::from_utf8_lossy(name).into_owned();
            if !in_root && !named.contains(&name.as_str()) {
                continue;
            }
            let Some(brace) = source.text[name_end..]
                .iter()
                .position(|b| *b == b'{')
                .map(|p| name_end + p)
            else {
                continue;
            };
            let close = delim_close(&source.text, brace);
            out.push(CoveredEnum {
                name,
                file: source.rel.clone(),
                line: line_of(&source.text, at),
                variants: variant_names(&source.text[brace + 1..close.saturating_sub(1)]),
            });
        }
    }
    out
}

/// Every `Ident::Ident` in production that is not a pattern and not a `use` path, as `(before,
/// after, source index)`.
fn path_occurrences(sources: &[Source]) -> BTreeSet<(String, String, usize)> {
    let mut out = BTreeSet::new();
    for (idx, source) in sources.iter().enumerate() {
        let text = &source.text;
        for at in find_all(text, b"::") {
            let mut start = at;
            while start > 0 && is_ident_byte(text[start - 1]) {
                start -= 1;
            }
            if start == at {
                continue;
            }
            let (after, after_end) = ident_at(text, at + 2);
            if after.is_empty() || after_end == at + 2 {
                continue;
            }
            if source.is_pattern_or_use(start) {
                continue;
            }
            out.insert((
                String::from_utf8_lossy(&text[start..at]).into_owned(),
                String::from_utf8_lossy(after).into_owned(),
                idx,
            ));
        }
    }
    out
}

/// Whether any production source builds `Enum::Variant` somewhere that is not a pattern and not a
/// `use` path.
fn variant_is_armed(tree: &Tree, covered: &CoveredEnum, variant: &str) -> bool {
    (0..tree.sources.len()).any(|idx| {
        tree.paths
            .contains(&(covered.name.clone(), variant.to_string(), idx))
            || (tree.sources[idx].rel == covered.file
                && tree
                    .paths
                    .contains(&("Self".to_string(), variant.to_string(), idx)))
    })
}

/// Everything the gates read, scanned once.
struct Tree {
    sources: Vec<Source>,
    declared: BTreeMap<String, (String, usize)>,
    rows: BTreeSet<String>,
    armed: BTreeSet<String>,
    covered: Vec<CoveredEnum>,
    paths: BTreeSet<(String, String, usize)>,
    calls: Vec<(usize, usize, String)>,
}

fn tree() -> &'static Tree {
    static TREE: OnceLock<Tree> = OnceLock::new();
    TREE.get_or_init(|| {
        let root = workspace_root();
        let sources = production_sources(&root);
        let declared = declared_codes(&root);
        let rows = registry_rows(&root);
        let armed = armed_codes(&sources);
        let covered = covered_enums(&sources);
        let paths = path_occurrences(&sources);
        let calls = constructor_calls(&sources);
        Tree {
            sources,
            declared,
            rows,
            armed,
            covered,
            paths,
            calls,
        }
    })
}

fn how_to_fix(what: &str, list: &str) -> String {
    format!(
        "\n\nEither construct it — {what} — or, if it is reserved on purpose, add a row to \
         `{list}` in crates/ply-span-tests/tests/armed.rs with a reason and a citation. \
         An entry there is not absolution: it is what makes \"reserved on purpose\" and \
         \"we forgot\" stop looking identical. Do NOT loosen the rule to make an entry \
         disappear; that inverts the point of this gate.\n\
         See CONTRIBUTING.md \u{a7}\"The shape it keeps taking: declared, registered, raised nowhere\"."
    )
}

#[test]
fn every_registered_code_is_constructed_in_production() {
    let Tree {
        sources,
        declared,
        armed,
        ..
    } = tree();

    assert!(
        sources.len() > 100,
        "scanned {} production files — the scan root is wrong and every assertion below \
         would pass over nothing",
        sources.len()
    );
    assert!(
        declared.len() > 50,
        "parsed {} codes out of ply_span::codes — the parser is broken",
        declared.len()
    );
    assert!(
        armed.len() > 50,
        "found {} armed codes — the constructor scan is broken",
        armed.len()
    );

    let allowed: BTreeSet<&str> = UNARMED_CODES.iter().map(|(name, _)| *name).collect();
    let mut dead: Vec<(&String, &(String, usize))> = declared
        .iter()
        .filter(|(name, _)| !armed.contains(*name) && !allowed.contains(name.as_str()))
        .collect();
    dead.sort_by_key(|(_, (number, _))| number.clone());

    if !dead.is_empty() {
        let mut message = format!(
            "{} registered diagnostic code(s) are declared and registered but constructed \
             nowhere in production source:\n",
            dead.len()
        );
        for (name, (number, line)) in &dead {
            let _ = writeln!(
                message,
                "  {number} {name}    declared at crates/ply-span/src/lib.rs:{line}"
            );
        }
        message.push_str(
            "\nA code is ARMED iff a production source calls Diagnostic::error(codes::NAME, ..) \
             or Diagnostic::warning(codes::NAME, ..), or passes codes::NAME to a wrapper listed \
             in CODE_INDIRECTION. A row in the registry table, an entry in \
             crates/ply-eval/src/host.rs's RESERVED_CODES, and any mention under #[cfg(test)] \
             or crates/*/tests/ are NOT armings.",
        );
        message.push_str(&how_to_fix(
            "raise it where the condition it names is detected",
            "UNARMED_CODES",
        ));
        panic!("{message}");
    }
}

#[test]
fn every_variant_of_a_covered_enum_is_constructed_in_production() {
    let tree = tree();
    let covered = &tree.covered;

    assert!(
        covered.len() > 10,
        "found {} covered enums — COVERED_ENUM_ROOTS resolved to nothing",
        covered.len()
    );
    let total: usize = covered.iter().map(|e| e.variants.len()).sum();
    assert!(
        total > 40,
        "found {total} variants across the covered enums"
    );

    let allowed: BTreeSet<&str> = UNARMED_VARIANTS.iter().map(|(name, _)| *name).collect();
    let mut dead = Vec::new();
    for enumeration in covered.iter() {
        for variant in &enumeration.variants {
            let key = format!("{}::{variant}", enumeration.name);
            if allowed.contains(key.as_str()) {
                continue;
            }
            if !variant_is_armed(tree, enumeration, variant) {
                dead.push((key, enumeration.file.clone(), enumeration.line));
            }
        }
    }
    dead.sort();
    dead.dedup();

    assert!(
        dead.len() < total,
        "every covered variant looks unarmed, which is a broken scan and not a finding"
    );

    if !dead.is_empty() {
        let mut message = format!(
            "{} variant(s) of a covered enum are declared and matched on but constructed \
             nowhere in production source:\n",
            dead.len()
        );
        for (key, file, line) in &dead {
            let _ = writeln!(message, "  {key}    declared at {file}:{line}");
        }
        message.push_str(
            "\nA variant is ARMED iff it appears in production source outside a pattern \
             position. A match arm, an `if let`, a `while let`, a `matches!` and a `use` path \
             are all consumers: they prove something reads the variant, never that anything \
             builds one.",
        );
        message.push_str(&how_to_fix(
            "build one where the condition it names occurs",
            "UNARMED_VARIANTS",
        ));
        panic!("{message}");
    }
}

/// Without this, one new pass-through wrapper disarms the rule for every code that only reaches the
/// constructor through it, and both gates go green over the gap.
#[test]
fn every_diagnostic_constructor_call_names_its_code_literally() {
    let Tree { sources, calls, .. } = tree();

    assert!(
        calls.len() > 200,
        "found {} Diagnostic::error/warning calls — the scan is broken",
        calls.len()
    );

    let mut unlisted = Vec::new();
    for (idx, at, arg) in calls.iter() {
        if code_from_path(arg).is_some() {
            continue;
        }
        let source = &sources[*idx];
        let function = enclosing_function(&source.text, *at);
        if CODE_INDIRECTION
            .iter()
            .any(|w| w.file == source.rel && w.function == function)
        {
            continue;
        }
        unlisted.push(format!(
            "  {}:{} in fn {function} — first argument is `{arg}`",
            source.rel,
            line_of(&source.text, *at)
        ));
    }

    assert!(
        unlisted.is_empty(),
        "{} Diagnostic constructor call(s) take their code indirectly, and are not in \
         CODE_INDIRECTION:\n{}\n\nEvery code that reaches Diagnostic only through an unlisted \
         wrapper is invisible to every_registered_code_is_constructed_in_production, which \
         would then report it dead — or, if the wrapper were quietly allowlisted by file, hide \
         a real death. Add the wrapper to CODE_INDIRECTION in \
         crates/ply-span-tests/tests/armed.rs with a reason, or pass codes::NAME literally.",
        unlisted.len(),
        unlisted.join("\n")
    );
}

/// The mirror defect: declared and *not* registered.
#[test]
fn the_code_registry_table_is_total_over_the_codes_module() {
    let Tree { declared, rows, .. } = tree();

    assert!(
        declared.len() > 50 && rows.len() > 50,
        "parsed {} constants and {} registry rows — one of the two parsers is broken",
        declared.len(),
        rows.len()
    );

    let missing: Vec<&String> = declared.keys().filter(|n| !rows.contains(*n)).collect();
    assert!(
        missing.is_empty(),
        "{} constant(s) in ply_span::codes have no row in the registry table in \
         crates/ply-span/src/lib.rs: {:?}\n\nA code with no row has no published number that \
         anything checks, so it can be renumbered without a test noticing. Add a row \
         (\"NAME\", codes::NAME, \"E0000\") to `let registry = [` — adding a row moves no \
         existing number.",
        missing.len(),
        missing
    );

    let stale: Vec<&String> = rows.iter().filter(|n| !declared.contains_key(*n)).collect();
    assert!(
        stale.is_empty(),
        "the registry table names {} constant(s) that ply_span::codes no longer declares: {:?}",
        stale.len(),
        stale
    );
}

/// An allowlist that outlives its reason is the same defect wearing the gate's own clothes.
#[test]
fn no_allowlist_entry_has_outlived_its_reason() {
    let tree = tree();
    let Tree {
        sources,
        declared,
        armed,
        covered,
        ..
    } = tree;

    let mut stale = Vec::new();

    for (name, reason) in UNARMED_CODES {
        assert!(
            reason.len() > 40,
            "UNARMED_CODES entry `{name}` has no real reason"
        );
        if !declared.contains_key(*name) {
            stale.push(format!(
                "  UNARMED_CODES has `{name}`, which ply_span::codes no longer declares — \
                 delete the row"
            ));
        } else if armed.contains(*name) {
            stale.push(format!(
                "  UNARMED_CODES has `{name}`, which production source now constructs — \
                 delete the row"
            ));
        }
    }

    for (key, reason) in UNARMED_VARIANTS {
        assert!(
            reason.len() > 40,
            "UNARMED_VARIANTS entry `{key}` has no real reason"
        );
        let Some((enum_name, variant)) = key.split_once("::") else {
            panic!("UNARMED_VARIANTS entry `{key}` is not `Enum::Variant`");
        };
        let declarations: Vec<&CoveredEnum> =
            covered.iter().filter(|e| e.name == enum_name).collect();
        if declarations.is_empty() {
            stale.push(format!(
                "  UNARMED_VARIANTS has `{key}`, but no covered enum is called `{enum_name}` \
                 any more — delete the row"
            ));
            continue;
        }
        if !declarations
            .iter()
            .any(|e| e.variants.iter().any(|v| v == variant))
        {
            stale.push(format!(
                "  UNARMED_VARIANTS has `{key}`, but `{enum_name}` has no variant \
                 `{variant}` any more — delete the row"
            ));
            continue;
        }
        if declarations
            .iter()
            .any(|e| variant_is_armed(tree, e, variant))
        {
            stale.push(format!(
                "  UNARMED_VARIANTS has `{key}`, which production source now constructs — \
                 delete the row"
            ));
        }
    }

    for wrapper in CODE_INDIRECTION {
        assert!(
            wrapper.reason.len() > 40,
            "CODE_INDIRECTION entry `{}::{}` has no real reason",
            wrapper.file,
            wrapper.function
        );
        let Some(source) = sources.iter().find(|s| s.rel == wrapper.file) else {
            stale.push(format!(
                "  CODE_INDIRECTION names {}, which is not a production source any more",
                wrapper.file
            ));
            continue;
        };
        let needle = format!("fn {}", wrapper.function).into_bytes();
        if !contains(&source.text, &needle) {
            stale.push(format!(
                "  CODE_INDIRECTION names fn {} in {}, which does not define it any more",
                wrapper.function, wrapper.file
            ));
        }
    }

    assert!(
        stale.is_empty(),
        "{} allowlist entry(s) no longer describe the tree:\n{}",
        stale.len(),
        stale.join("\n")
    );
}

/// Two covered enums with the same name cannot be told apart by a lexical scan.
#[test]
fn ambiguous_enum_names_are_declared() {
    let covered = &tree().covered;

    let mut counts: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for enumeration in covered.iter() {
        counts
            .entry(enumeration.name.as_str())
            .or_default()
            .push(format!("{}:{}", enumeration.file, enumeration.line));
    }
    let declared: BTreeSet<&str> = AMBIGUOUS_ENUM_NAMES.iter().map(|(name, _)| *name).collect();

    let undeclared: Vec<String> = counts
        .iter()
        .filter(|(name, sites)| sites.len() > 1 && !declared.contains(*name))
        .map(|(name, sites)| format!("  {name} — {}", sites.join(", ")))
        .collect();
    assert!(
        undeclared.is_empty(),
        "{} covered enum name(s) are declared more than once, so `Name::Variant` cannot be \
         attributed to one of them and a hit arms the variant in all of them:\n{}\n\nRename one, \
         or add it to AMBIGUOUS_ENUM_NAMES with the reason the false negative is acceptable.",
        undeclared.len(),
        undeclared.join("\n")
    );

    let gone: Vec<&str> = AMBIGUOUS_ENUM_NAMES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| counts.get(name).is_none_or(|sites| sites.len() < 2))
        .collect();
    assert!(
        gone.is_empty(),
        "AMBIGUOUS_ENUM_NAMES lists {gone:?}, which is no longer ambiguous — delete the row"
    );
}

fn mask_of(src: &str) -> Vec<bool> {
    let text = blank_cfg_test_blocks(&blank_literals_and_comments(src.as_bytes()));
    let mut masked = vec![false; text.len()];
    for (from, to) in pattern_and_use_regions(&text) {
        for m in &mut masked[from.min(text.len())..to.min(text.len())] {
            *m = true;
        }
    }
    masked
}

#[track_caller]
fn assert_pattern(src: &str, needle: &str, expected: bool) {
    let text = blank_cfg_test_blocks(&blank_literals_and_comments(src.as_bytes()));
    let at = find_all(&text, needle.as_bytes())
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("`{needle}` does not survive blanking in:\n{src}"));
    assert_eq!(
        mask_of(src)[at],
        expected,
        "`{needle}` should {} be a pattern position in:\n{src}",
        if expected { "" } else { "not" }
    );
}

#[test]
fn arm_bodies_are_expressions_and_arm_heads_are_patterns() {
    // The exact shape that fooled the prototype: a block-bodied arm with no trailing comma,
    // followed by another arm.
    assert_pattern(
        "fn f(e: E) { match e { E::A => { g(); } E::B => { h(); } } }",
        "E::B",
        true,
    );
    assert_pattern(
        "fn f(e: E) { match e { E::A => E::B, _ => x } }",
        "E::B",
        false,
    );
    assert_pattern(
        "fn f(e: E) { match e { E::A => E::B, _ => x } }",
        "E::A",
        true,
    );
    // A nested match inside an arm body is still scanned.
    assert_pattern(
        "fn f(e: E) { match e { E::A => match g() { E::C => 1, _ => 2 }, _ => 3 } }",
        "E::C",
        true,
    );
    // A guard is an expression, not a pattern.
    assert_pattern(
        "fn f(e: E) { match e { x if x == E::A => 1, _ => 2 } }",
        "E::A",
        false,
    );
    // Or-patterns and struct patterns.
    assert_pattern(
        "fn f(e: E) { match e { E::A | E::B => 1, _ => 2 } }",
        "E::B",
        true,
    );
    assert_pattern(
        "fn f(e: E) { match e { E::A { x } => x, _ => 2 } }",
        "E::A",
        true,
    );
    // The other consumers.
    assert_pattern("fn f(e: E) { if let E::A = e { g() } }", "E::A", true);
    assert_pattern("fn f(e: E) { while let E::A = e { g() } }", "E::A", true);
    assert_pattern("fn f(e: E) -> bool { matches!(e, E::A) }", "E::A", true);
    assert_pattern("use crate::e::E::A;", "E::A", true);
    // Producers.
    assert_pattern("fn f() -> E { E::A }", "E::A", false);
    assert_pattern("fn f(v: &mut Vec<E>) { v.push(E::A); }", "E::A", false);
    assert_pattern("fn f() -> E { E::A { x: 1 } }", "E::A", false);
    assert_pattern("fn f() { let x = E::A; }", "E::A", false);
}

#[test]
fn comments_and_literals_are_not_source() {
    let src = "/// codes::GHOST and E::Ghost\nfn f() { let s = \"codes::GHOST\"; }";
    let text = blank_literals_and_comments(src.as_bytes());
    assert_eq!(text.len(), src.len(), "blanking changed the byte offsets");
    assert!(
        !contains(&text, b"codes::GHOST"),
        "a doc comment or a string literal survived blanking"
    );
    assert!(!contains(&text, b"E::Ghost"));

    let raw = "fn f() { let s = r#\"codes::GHOST\"#; let c = '\\''; let d = 'é'; }";
    let text = blank_literals_and_comments(raw.as_bytes());
    assert_eq!(text.len(), raw.len());
    assert!(!contains(&text, b"codes::GHOST"), "a raw string survived");
    // A lifetime is not a character literal.
    let lifetime = "fn f<'a>(x: &'a str) -> E { E::A }";
    let text = blank_literals_and_comments(lifetime.as_bytes());
    assert!(
        contains(&text, b"E::A"),
        "a lifetime was mistaken for a character literal and ate the rest of the file"
    );

    let nested = "/* a /* b */ codes::GHOST */ fn f() {}";
    assert!(!contains(
        &blank_literals_and_comments(nested.as_bytes()),
        b"codes::GHOST"
    ));
}

#[test]
fn cfg_test_items_and_modules_are_not_production() {
    let src = "fn a() {}\n#[cfg(test)]\nmod tests {\n    use codes::GHOST;\n}\nfn b() {}";
    let text = blank_cfg_test_blocks(src.as_bytes());
    assert_eq!(text.len(), src.len());
    assert!(!contains(&text, b"GHOST"), "a #[cfg(test)] block survived");
    assert!(contains(&text, b"fn a()") && contains(&text, b"fn b()"));

    // A `#[cfg(test)] mod x;` declaration must survive: the resolver reads it.
    let decl = "#[cfg(test)]\nmod tests;\nfn a() {}";
    assert!(contains(
        &blank_cfg_test_blocks(decl.as_bytes()),
        b"mod tests;"
    ));
    assert_eq!(
        mod_declarations(decl.as_bytes()),
        vec![("tests".to_string(), true)]
    );
    assert_eq!(
        mod_declarations(b"pub mod slice;\nmod key;"),
        vec![("slice".to_string(), false), ("key".to_string(), false)]
    );
    assert_eq!(
        mod_declarations(b"#[cfg(not(test))]\npub(crate) mod real;"),
        vec![("real".to_string(), false)]
    );
    // A `mod x { .. }` with a body is not a file and must not be resolved.
    assert!(mod_declarations(b"mod inline { fn f() {} }").is_empty());
}

/// The walk from `#[cfg(test)]` to the item's body used to stop at the first `;`, which a header
/// can contain without ending anything.
#[test]
fn a_cfg_test_item_is_not_production_whatever_its_header_looks_like() {
    // A `;` inside an array type in the return position.
    let array_return =
        "#[cfg(test)]\nfn kept() -> [usize; CLASSES] {\n    codes::GHOST\n}\nfn after() {}";
    let text = blank_cfg_test_blocks(array_return.as_bytes());
    assert_eq!(text.len(), array_return.len(), "blanking moved the offsets");
    assert!(
        !contains(&text, b"GHOST"),
        "a #[cfg(test)] fn survived because its return type held a `;`"
    );
    assert!(contains(&text, b"fn after()"), "blanking ran past the item");

    // The same `;` in an argument type.
    assert!(!contains(
        &blank_cfg_test_blocks(b"#[cfg(test)]\nfn f(a: [u8; 4]) {\n    codes::GHOST\n}"),
        b"GHOST"
    ));

    // `;`-terminated items that are not modules are test-only source too, and a `const` initialiser
    // is a construction wherever it is written.
    for src in [
        "#[cfg(test)]\nconst K: E = E::GHOST;\nfn after() {}",
        "#[cfg(test)]\nstatic K: E = E::GHOST;\nfn after() {}",
        "#[cfg(test)]\nuse crate::e::E::GHOST;\nfn after() {}",
    ] {
        let text = blank_cfg_test_blocks(src.as_bytes());
        assert_eq!(text.len(), src.len());
        assert!(
            !contains(&text, b"GHOST"),
            "a #[cfg(test)] item survived: {src}"
        );
        assert!(
            contains(&text, b"fn after()"),
            "blanking ran past the item: {src}"
        );
    }

    // ... and a `mod` declaration is still the exception, however it is spelled, because the
    // resolver reads it to decide the file it names is test-only.
    for src in [
        "#[cfg(test)]\nmod tests;",
        "#[cfg(test)]\npub(crate) mod delta_tests;",
    ] {
        assert!(
            contains(&blank_cfg_test_blocks(src.as_bytes()), b"mod "),
            "the resolver's `mod` declaration was blanked: {src}"
        );
    }
    assert!(header_declares_a_module(b" pub(crate) mod tests"));
    assert!(!header_declares_a_module(b" const MODE: u8 = 1"));
}

#[test]
fn a_variant_list_is_read_off_an_enum_body() {
    let src = "#[derive(Clone)]\npub enum E {\n    /// d\n    A,\n    B { x: u8 },\n    C(Vec<(u8, u8)>),\n    D = 7,\n}";
    let text = blank_literals_and_comments(src.as_bytes());
    let brace = find_all(&text, b"pub enum E")[0]
        + text[find_all(&text, b"pub enum E")[0]..]
            .iter()
            .position(|b| *b == b'{')
            .expect("the enum has a body");
    let close = delim_close(&text, brace);
    assert_eq!(
        variant_names(&text[brace + 1..close - 1]),
        vec!["A", "B", "C", "D"]
    );
}

#[test]
fn a_codes_path_is_recognised_however_it_is_qualified() {
    assert_eq!(code_from_path("codes::X"), Some("X"));
    assert_eq!(code_from_path("crate::codes::X"), Some("X"));
    assert_eq!(code_from_path("ply_span::codes::X"), Some("X"));
    assert_eq!(code_from_path("code"), None);
    assert_eq!(code_from_path("self.code"), None);
    assert_eq!(code_from_path("other::X"), None);
}

/// Production files that install a compiled backend, and the reason each is allowed to, in the
/// shape `UNARMED_CODES` uses and for the same reason: a route that appears without anybody
/// deciding it should must look different from one that was decided on.
const BACKEND_INSTALLERS: &[(&str, &str)] = &[
    (
        "crates/ply-test/src/lib.rs",
        "`InterpExecutor::machine_lowering`, installing what `InterpExecutor::with_backend` \
         was handed. It is the route `ply test` has, and it arms the cache rule on it \
         twice: `cache_bypassed` reads `args.backend` so nothing is read, and `run_with` \
         records `Record::Backend` so nothing is written. \
         crates/ply-cli/tests/suite/backend.rs holds both, each seen to fail.",
    ),
    (
        "crates/ply-cli/src/commands/run.rs",
        "`evaluate`, installing what `ply run --backend` names on the machine that runs \
         `main`. `ply run` has no result cache: nothing it answers is a `Pass`, nothing is \
         read before `main` and nothing is recorded after it, so neither half of the rule \
         has a route to break. `run_attaches_a_backend_to_main_and_refuses_a_spec_it_cannot_parse` \
         in crates/ply-cli/tests/suite/backend.rs runs `main` under both backends and was \
         seen to fail, on the seam memo walking a scalar answer for parts.",
    ),
    (
        "crates/ply-cli/src/artifact.rs",
        "`evaluate`, the same flag over a `.plyx`: the artifact's verified definitions run \
         under the backend, and an artifact run has no result cache either.",
    ),
];

/// A backend installed by a route the cache rule does not know about.
#[test]
fn a_shipping_command_that_installs_a_backend_must_also_bypass_the_cache() {
    let Tree { sources, .. } = tree();
    assert!(
        sources.len() > 100,
        "scanned {} production files — the scan root is wrong",
        sources.len()
    );

    let installers: BTreeSet<&str> = sources
        .iter()
        .filter(|s| contains(&s.text, b".set_compiled("))
        .map(|s| s.rel.as_str())
        .collect();
    let listed: BTreeSet<&str> = BACKEND_INSTALLERS.iter().map(|(path, _)| *path).collect();

    let unlisted: Vec<&&str> = installers.difference(&listed).collect();
    assert!(
        unlisted.is_empty(),
        "{unlisted:?} installs a compiled backend and is not listed in BACKEND_INSTALLERS.\n\n\
             A run with a backend attached is a third execution strategy, and a cached `Pass` is \
             a claim about the authoritative engine. Every route that can install one owes both \
             halves of the cache rule: the command must not READ the cache (a clause \
             `cache_bypassed` can see) and the runner must not WRITE it (`Record::Backend`).\n\
             Add the route here with the reason it is safe, and a test that has been seen to \
             fail. Do NOT loosen this gate to make the entry disappear."
    );
    let stale: Vec<&&str> = listed.difference(&installers).collect();
    assert!(
        stale.is_empty(),
        "BACKEND_INSTALLERS lists {stale:?}, which no longer installs a backend. Delete the \
         row — an excuse that outlives its fact is what this file exists to prevent."
    );

    // The two halves, read off the production source that owes them.
    let cli = sources
        .iter()
        .find(|s| s.rel == "crates/ply-cli/src/commands/test.rs")
        .expect("ply test is production source");
    let bypassed = between(&cli.text, b"fn cache_bypassed(", b"\n}");
    assert!(
        contains(&bypassed, b"backend"),
        "`cache_bypassed` cannot see whether a backend was installed, so a backend run would \
         read the result cache: the cache rule.\ncache_bypassed reads: {}",
        String::from_utf8_lossy(&bypassed)
    );

    let runner = sources
        .iter()
        .find(|s| s.rel == "crates/ply-test/src/lib.rs")
        .expect("the runner is production source");
    assert!(
        contains(&runner.text, b"Record::Backend"),
        "the runner no longer records `Record::Backend`, so a test that entered native code can \
         have its pass written. That is the half of the rule that survives a backend arriving by \
         a route no flag names."
    );
}

/// The text between the first `open` and the next `close` after it.
fn between(text: &[u8], open: &[u8], close: &[u8]) -> Vec<u8> {
    let Some(&from) = find_all(text, open).first() else {
        return Vec::new();
    };
    let rest = &text[from..];
    match find_all(rest, close).first() {
        Some(&to) => rest[..to].to_vec(),
        None => rest.to_vec(),
    }
}

/// Two decision records may not share a number.
#[test]
fn no_two_adrs_share_a_number() {
    let dir = workspace_root().join("docs/adr");
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("docs/adr must be readable") {
        let name = entry.expect("a readable entry").file_name();
        let name = name.to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".md") else {
            continue;
        };
        let Some((number, _)) = stem.split_once('-') else {
            panic!("`docs/adr/{name}` is not `NNNN-slug.md`, so nothing can order it");
        };
        seen.entry(number.to_string()).or_default().push(name);
    }
    assert!(
        !seen.is_empty(),
        "no records found — this test is reading the wrong directory"
    );
    let clashes: Vec<String> = seen
        .iter()
        .filter(|(_, files)| files.len() > 1)
        .map(|(n, files)| format!("{n}: {}", files.join(", ")))
        .collect();
    assert!(
        clashes.is_empty(),
        "two records share a number, so a reference to that number is ambiguous \
         and one of them has to be renumbered:\n  {}",
        clashes.join("\n  ")
    );
}
