//! Which tables a statement touches, and a refusal for every statement whose
//! answer this cannot compute.
//!
//! This is the file the milestone is about. A `db.query[orders]` call site
//! writes one label; the statement may join three tables, and nothing in the
//! type system can see it because the SQL is a `String`. So the driver computes
//! the table set from the text, once per statement at prepare time, and the
//! answer is what refuses an undeclared table before a row moves and what the
//! machine checks the handler's report against afterwards.
//!
//! Two rules decide every line below:
//!
//! - **Conservative in the safe direction, by construction.** A construct this
//!   scanner does not model is [`codes::DB_STATEMENT_REFUSED`] naming the byte
//!   offset and the token — never an empty table set and never a silent skip. A
//!   defect here is a refusal to run rather than a footprint that under-reports,
//!   because an under-reporting footprint corrupts scheduling and isolation with
//!   a green result rather than a red one. The residual is a defect that
//!   *mis*-recognises a construct, which can still under-report; that is why the
//!   scanner is disclosed in `ply hosts` and why the differential test against
//!   `EXPLAIN (GENERIC_PLAN)` exists.
//! - **Over-reporting costs concurrency; under-reporting costs correctness.** A
//!   partition the planner prunes, a branch that is constant-false, an arm of a
//!   `UNION` that returns nothing — all of them are tables this names and
//!   `EXPLAIN` does not, and that is the direction to be wrong in.
//!
//! It is Rust rather than Ply for one reason that does not apply to W3's HTTP
//! framing: the driver needs the answer and the driver is Rust, so a Ply
//! implementation would mean two scanners, and two parsers that disagree is
//! exactly the hazard being avoided — here the disagreement would be between the
//! footprint a test observes and the footprint the scheduler was given.

use ply_span::{Diagnostic, Span, codes};
use std::collections::BTreeSet;
use std::fmt;

/// Statement shapes W4 admits. Everything else is a refusal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Select,
    Insert,
    Update,
    Delete,
    Values,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Select => "select",
            Kind::Insert => "insert",
            Kind::Update => "update",
            Kind::Delete => "delete",
            Kind::Values => "values",
        }
    }

    /// Whether the statement can change a row. `query` over one of these is the
    /// call site claiming a read of something that writes, and the driver
    /// refuses it rather than recording a `read` atom for a `write`.
    pub fn writes(self) -> bool {
        matches!(self, Kind::Insert | Kind::Update | Kind::Delete)
    }
}

/// The accepted statement set, as `ply hosts` prints it. A reviewer reads this
/// line to know what the trusted computing base will run at all.
pub const ACCEPTED: &str = "select insert update delete values with";

/// What one statement touches.
///
/// Split by mode rather than pooled, because an `update … from other` writes one
/// relation and reads another, and a row that called both writes would serialise
/// every reader of `other` against it for nothing.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Tables {
    /// Relations the statement can change. At most one for the shapes admitted,
    /// plus any a data-modifying CTE names.
    pub written: BTreeSet<String>,
    /// Relations it reads and does not change.
    pub read: BTreeSet<String>,
}

impl Tables {
    /// Every relation, in one set, for the places that only need "which tables".
    pub fn all(&self) -> BTreeSet<String> {
        self.written.union(&self.read).cloned().collect()
    }

    fn write(&mut self, name: String) {
        self.read.remove(&name);
        self.written.insert(name);
    }

    fn read(&mut self, name: String) {
        if !self.written.contains(&name) {
            self.read.insert(name);
        }
    }

    fn absorb(&mut self, other: Tables) {
        for name in other.written {
            self.write(name);
        }
        for name in other.read {
            self.read(name);
        }
    }
}

/// One statement's scan.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Scan {
    pub kind: Kind,
    pub tables: Tables,
}

impl fmt::Display for Scan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let all: Vec<String> = self.tables.all().into_iter().collect();
        write!(f, "{} over {}", self.kind.as_str(), all.join(", "))
    }
}

/// Functions whose value is not a function of the program's state, refused in
/// statement text.
///
/// The workaround is the point rather than an inconvenience: a timestamp comes
/// from `clock.now()` **as a parameter**, which puts the nondeterminism in the
/// row where `E0412` can see it instead of hiding it inside a string. A `now()`
/// spliced into SQL is a `det` test that reads a different value on every run
/// and a compiler that cannot tell.
const NONDETERMINISTIC: &[&str] = &[
    "now",
    "random",
    "current_timestamp",
    "current_date",
    "current_time",
    "localtime",
    "localtimestamp",
    "clock_timestamp",
    "statement_timestamp",
    "transaction_timestamp",
    "timeofday",
    "gen_random_uuid",
];

/// The functions a statement may call.
///
/// An allowlist rather than a denylist, and that is the whole of §2.4's "refuses
/// everything else" applied one level below the statement shape. A function call
/// is the one place an admitted statement can reach outside its own table set:
/// `set_config` rebinds the `search_path` every later borrower of the pooled
/// connection inherits, `pg_advisory_lock` holds a session lock the next borrower
/// blocks on, `pg_read_file` reads the server's disk — and every one of them
/// reports the footprint of the relation it was selected `from`, which is what
/// two read-only endpoints are scheduled side by side on.
///
/// Everything here is a function of its arguments alone: no session state, no
/// catalogue, no clock, no relation. Adding a name is a change to the trusted
/// computing base and a line in a diff.
const CALLABLE: &[&str] = &[
    // Aggregates the scanner models a footprint for.
    "avg", "count", "max", "min", "sum",
    // Conditional expressions, which postgres spells as calls.
    "coalesce", "greatest", "least", "nullif",
    // Numeric.
    "abs", "ceil", "ceiling", "div", "exp", "floor", "ln", "log", "mod", "power", "round", "sign",
    "sqrt", "trunc",
    // Text.
    "btrim", "char_length", "character_length", "concat", "concat_ws", "decode", "encode", "left",
    "length", "lower", "lpad", "ltrim", "md5", "octet_length", "overlay", "position", "repeat",
    "replace", "reverse", "right", "rpad", "rtrim", "split_part", "starts_with", "strpos", "substr",
    "substring", "translate", "trim", "upper",
    // json / jsonb, encode-side only: every set-returning one is a `from` item
    // and is refused there.
    "json_build_array", "json_build_object", "jsonb_array_length", "jsonb_build_array",
    "jsonb_build_object", "jsonb_extract_path", "jsonb_extract_path_text", "jsonb_strip_nulls",
    "jsonb_typeof", "to_json", "to_jsonb",
];

/// Words that may precede a `(` without being a function call.
///
/// Syntax rather than functions: `array(select …)` is a subquery constructor,
/// `cast(x as int)` is a cast, `exists (…)` and `row(…)` are expressions. Every
/// other spelling is covered by [`is_reserved`], which is where `not (…)`,
/// `in (…)` and `case … end` are already keywords.
const SYNTACTIC: &[&str] = &["array", "cast", "exists", "row", "any", "some", "nullif"];

/// Words that end an expression at depth zero. Collected once so that a clause
/// added to the grammar cannot be added to the parser and forgotten here.
const CLAUSE_WORDS: &[&str] = &[
    "from",
    "where",
    "group",
    "having",
    "window",
    "order",
    "limit",
    "offset",
    "fetch",
    "for",
    "union",
    "intersect",
    "except",
    "returning",
    "using",
    "on",
    "set",
    "values",
    "join",
    "inner",
    "left",
    "right",
    "full",
    "cross",
    "natural",
    "as",
];

pub fn scan(sql: &str, span: Span) -> Result<Scan, Diagnostic> {
    let tokens = tokenize(sql, span)?;
    let mut parser = Parser {
        sql,
        tokens,
        at: 0,
        span,
        depth: 0,
    };
    let scan = parser.statement()?;
    match parser.peek() {
        None => Ok(scan),
        Some(t) if t.kind == TokenKind::Semicolon => {
            let start = t.start;
            Err(parser.stacked(start))
        }
        Some(t) => Err(parser.refuse(t.start, &t.display(sql), "the statement ends here")),
    }
}

// --- Tokens -----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TokenKind {
    /// An identifier or a keyword. `text` is folded to lower case unless it was
    /// double-quoted, which is postgres's own rule and the reason `"Items"` and
    /// `items` are different relations.
    Word,
    Number,
    /// A string literal, in any of postgres's spellings. Its contents are never
    /// looked at: the whole point is that a `;` inside one is not a statement
    /// separator.
    Text,
    /// `$1`. Not a value: the driver never sees one become syntax.
    Placeholder,
    Punct,
    /// Its own kind because it is the one token whose presence is always a
    /// refusal, and the message for it is specific.
    Semicolon,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    /// For a `Word`, the folded name; for a `Punct`, the punctuation itself.
    text: String,
    quoted: bool,
    start: usize,
    end: usize,
}

impl Token {
    fn display(&self, sql: &str) -> String {
        match self.kind {
            TokenKind::Semicolon => ";".to_string(),
            _ => sql[self.start..self.end].to_string(),
        }
    }

    fn is_word(&self, word: &str) -> bool {
        self.kind == TokenKind::Word && !self.quoted && self.text == word
    }

    fn is_punct(&self, p: &str) -> bool {
        self.kind == TokenKind::Punct && self.text == p
    }
}

fn tokenize(sql: &str, span: Span) -> Result<Vec<Token>, Diagnostic> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c => {
                i += 1;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                // Postgres nests block comments, so a naive scan to the first
                // `*/` would leave the tail of an outer comment as statement
                // text — which is a construct the parser would then refuse, but
                // for the wrong reason and at the wrong offset.
                let start = i;
                let mut nesting = 1usize;
                i += 2;
                while i < bytes.len() && nesting > 0 {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        nesting += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        nesting -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if nesting > 0 {
                    return Err(unterminated(span, start, "a block comment"));
                }
            }
            b';' => {
                out.push(Token {
                    kind: TokenKind::Semicolon,
                    text: ";".into(),
                    quoted: false,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'\'' => {
                let start = i;
                i = string_literal(bytes, i, span, false)?;
                out.push(text_token(start, i));
            }
            b'e' | b'E' if bytes.get(i + 1) == Some(&b'\'') => {
                let start = i;
                i = string_literal(bytes, i + 1, span, true)?;
                out.push(text_token(start, i));
            }
            b'b' | b'B' | b'x' | b'X' if bytes.get(i + 1) == Some(&b'\'') => {
                let start = i;
                i = string_literal(bytes, i + 1, span, false)?;
                out.push(text_token(start, i));
            }
            b'u' | b'U' if bytes.get(i + 1) == Some(&b'&') && bytes.get(i + 2) == Some(&b'\'') => {
                let start = i;
                i = string_literal(bytes, i + 2, span, false)?;
                out.push(text_token(start, i));
            }
            b'"' => {
                let start = i;
                let mut j = i + 1;
                let mut name = String::new();
                loop {
                    match bytes.get(j) {
                        None => return Err(unterminated(span, start, "a quoted identifier")),
                        Some(b'"') if bytes.get(j + 1) == Some(&b'"') => {
                            name.push('"');
                            j += 2;
                        }
                        Some(b'"') => {
                            j += 1;
                            break;
                        }
                        Some(_) => {
                            let ch = sql[j..].chars().next().expect("a char boundary");
                            name.push(ch);
                            j += ch.len_utf8();
                        }
                    }
                }
                out.push(Token {
                    kind: TokenKind::Word,
                    text: name,
                    quoted: true,
                    start,
                    end: j,
                });
                i = j;
            }
            b'$' => {
                if bytes.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
                    let start = i;
                    let mut j = i + 1;
                    while bytes.get(j).is_some_and(u8::is_ascii_digit) {
                        j += 1;
                    }
                    out.push(Token {
                        kind: TokenKind::Placeholder,
                        text: sql[start..j].to_string(),
                        quoted: false,
                        start,
                        end: j,
                    });
                    i = j;
                } else {
                    let start = i;
                    i = dollar_quoted(sql, i, span)?;
                    out.push(text_token(start, i));
                }
            }
            _ if c.is_ascii_digit() => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
                {
                    i += 1;
                }
                out.push(Token {
                    kind: TokenKind::Number,
                    text: sql[start..i].to_string(),
                    quoted: false,
                    start,
                    end: i,
                });
            }
            _ if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 => {
                let start = i;
                while i < bytes.len() {
                    let b = bytes[i];
                    if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80 {
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push(Token {
                    kind: TokenKind::Word,
                    text: sql[start..i].to_ascii_lowercase(),
                    quoted: false,
                    start,
                    end: i,
                });
            }
            _ => {
                out.push(Token {
                    kind: TokenKind::Punct,
                    text: (c as char).to_string(),
                    quoted: false,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
        }
    }
    Ok(out)
}

fn text_token(start: usize, end: usize) -> Token {
    Token {
        kind: TokenKind::Text,
        text: String::new(),
        quoted: false,
        start,
        end,
    }
}

/// From the opening quote to just past the closing one. `''` is an escaped
/// quote in every spelling; `\'` is one only in an `E''` string, and treating it
/// as one elsewhere would end a literal early and hand the rest of it to the
/// parser as statement text.
fn string_literal(
    bytes: &[u8],
    open: usize,
    span: Span,
    backslash_escapes: bool,
) -> Result<usize, Diagnostic> {
    let mut j = open + 1;
    loop {
        match bytes.get(j) {
            None => return Err(unterminated(span, open, "a string literal")),
            Some(b'\\') if backslash_escapes => j += 2,
            Some(b'\'') if bytes.get(j + 1) == Some(&b'\'') => j += 2,
            Some(b'\'') => return Ok(j + 1),
            Some(_) => j += 1,
        }
    }
}

/// `$tag$ … $tag$`, where the tag may be empty. The body is opaque, which is
/// what makes a `;` inside a function body not a statement separator.
fn dollar_quoted(sql: &str, open: usize, span: Span) -> Result<usize, Diagnostic> {
    let bytes = sql.as_bytes();
    let mut j = open + 1;
    while bytes
        .get(j)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        j += 1;
    }
    if bytes.get(j) != Some(&b'$') {
        return Err(unterminated(span, open, "a dollar-quoted string"));
    }
    let tag = &sql[open..=j];
    let body = j + 1;
    match sql[body..].find(tag) {
        Some(offset) => Ok(body + offset + tag.len()),
        None => Err(unterminated(span, open, "a dollar-quoted string")),
    }
}

// --- The parser -------------------------------------------------------------

/// A statement nested this deep is a program the scanner will not vouch for,
/// and a bound here is what keeps a pathological input from recursing the host's
/// own stack.
const MAX_NESTING: usize = 32;

struct Parser<'a> {
    sql: &'a str,
    tokens: Vec<Token>,
    at: usize,
    span: Span,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn peek_at(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.at + n)
    }

    fn advance(&mut self) {
        self.at += 1;
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self.peek().is_some_and(|t| t.is_word(word)) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.peek().is_some_and(|t| t.is_punct(p)) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn at_word(&self, word: &str) -> bool {
        self.peek().is_some_and(|t| t.is_word(word))
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), Diagnostic> {
        if self.eat_punct(p) {
            return Ok(());
        }
        Err(self.here(&format!("`{p}` was expected here")))
    }

    /// The statement, with its `WITH` prefix if it has one.
    fn statement(&mut self) -> Result<Scan, Diagnostic> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            return Err(self.here("this statement nests deeper than the scanner will follow"));
        }
        self.reject_nondeterministic()?;
        let mut ctes = BTreeSet::new();
        let mut tables = Tables::default();
        if self.eat_word("with") {
            let recursive = self.eat_word("recursive");
            loop {
                let name = self.identifier("a common table expression's name")?;
                // A `recursive` CTE names itself inside its own body, so the
                // name has to be in scope before the body is walked or the
                // self-reference reads as a relation the database does not have.
                if recursive {
                    ctes.insert(name.clone());
                }
                if self.eat_punct("(") {
                    self.name_list()?;
                }
                if !self.eat_word("as") {
                    return Err(self.here("`as` was expected after the name"));
                }
                self.eat_word("not");
                self.eat_word("materialized");
                self.expect_punct("(")?;
                let inner = self.statement_within(&ctes)?;
                self.expect_punct(")")?;
                tables.absorb(inner.tables);
                // Resolved to its own sources: a later reference to this name is
                // the CTE and not a relation, and the relations it read are
                // already in the set.
                ctes.insert(name);
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        let mut scan = self.body(&ctes)?;
        scan.tables.absorb(tables);
        self.depth -= 1;
        Ok(scan)
    }

    /// A nested statement, which inherits the enclosing `WITH`'s names.
    fn statement_within(&mut self, outer: &BTreeSet<String>) -> Result<Scan, Diagnostic> {
        let mut scan = self.statement()?;
        for name in outer {
            scan.tables.written.remove(name);
            scan.tables.read.remove(name);
        }
        Ok(scan)
    }

    fn body(&mut self, ctes: &BTreeSet<String>) -> Result<Scan, Diagnostic> {
        let Some(token) = self.peek() else {
            return Err(self.refuse(self.sql.len(), "", "this statement is empty"));
        };
        if token.kind == TokenKind::Semicolon {
            let start = token.start;
            return Err(self.stacked(start));
        }
        if token.is_word("select") || token.is_punct("(") {
            let tables = self.select_stmt(ctes)?;
            return Ok(Scan {
                kind: Kind::Select,
                tables,
            });
        }
        if token.is_word("values") {
            let tables = self.select_stmt(ctes)?;
            return Ok(Scan {
                kind: Kind::Values,
                tables,
            });
        }
        if token.is_word("insert") {
            return self.insert(ctes);
        }
        if token.is_word("update") {
            return self.update(ctes);
        }
        if token.is_word("delete") {
            return self.delete(ctes);
        }
        let start = token.start;
        let text = token.display(self.sql);
        Err(self.refuse(
            start,
            &text,
            "W4 runs `select`, `insert`, `update`, `delete` and `values`, with an optional `with`",
        ))
    }

    // --- select ---

    fn select_stmt(&mut self, ctes: &BTreeSet<String>) -> Result<Tables, Diagnostic> {
        let mut tables = self.select_core(ctes)?;
        loop {
            if self.at_word("union") || self.at_word("intersect") || self.at_word("except") {
                self.advance();
                self.eat_word("all");
                self.eat_word("distinct");
                tables.absorb(self.select_core(ctes)?);
                continue;
            }
            break;
        }
        // The tail clauses hold expressions and nothing that introduces a
        // relation of its own except a scalar subquery, which `skip_expr`
        // recurses into.
        if self.eat_word("order") {
            self.expect_word("by")?;
            self.skip_expr(
                ctes,
                &mut tables,
                &["limit", "offset", "fetch", "for"],
                false,
            )?;
        }
        loop {
            if self.eat_word("limit") || self.eat_word("offset") {
                self.skip_expr(
                    ctes,
                    &mut tables,
                    &["limit", "offset", "fetch", "for"],
                    false,
                )?;
                continue;
            }
            if self.at_word("fetch") || self.at_word("for") {
                // `FOR UPDATE` takes a row lock, which is a write in every sense
                // that matters to a conflict graph, and the scanner has no way
                // to say so through a `read` atom.
                let t = self.peek().expect("checked");
                let (start, text) = (t.start, t.display(self.sql));
                return Err(self.refuse(
                    start,
                    &text,
                    "`fetch` and the row-locking clauses are outside the statement set W4 admits",
                ));
            }
            break;
        }
        Ok(tables)
    }

    fn select_core(&mut self, ctes: &BTreeSet<String>) -> Result<Tables, Diagnostic> {
        let mut tables = Tables::default();
        if self.eat_punct("(") {
            let inner = self.select_stmt(ctes)?;
            self.expect_punct(")")?;
            tables.absorb(inner);
            return Ok(tables);
        }
        if self.eat_word("values") {
            loop {
                self.expect_punct("(")?;
                let mut depth = 1usize;
                while depth > 0 {
                    let Some(t) = self.peek() else {
                        return Err(self.here("this row is never closed"));
                    };
                    if t.is_punct("(") {
                        depth += 1;
                    } else if t.is_punct(")") {
                        depth -= 1;
                    } else if t.kind == TokenKind::Semicolon {
                        let start = t.start;
                        return Err(self.stacked(start));
                    }
                    self.check_call()?;
                    self.advance();
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
            return Ok(tables);
        }
        self.expect_word("select")?;
        self.eat_word("all");
        if self.eat_word("distinct") && self.eat_word("on") {
            self.expect_punct("(")?;
            self.skip_parens(ctes, &mut tables)?;
        }
        self.target_list(ctes, &mut tables)?;
        if self.eat_word("from") {
            self.source_list(ctes, &mut tables)?;
        }
        if self.eat_word("where") {
            self.skip_expr(ctes, &mut tables, CLAUSE_WORDS, false)?;
        }
        if self.eat_word("group") {
            self.expect_word("by")?;
            self.skip_expr(ctes, &mut tables, CLAUSE_WORDS, false)?;
        }
        if self.eat_word("having") {
            self.skip_expr(ctes, &mut tables, CLAUSE_WORDS, false)?;
        }
        if self.at_word("window") {
            let t = self.peek().expect("checked");
            let (start, text) = (t.start, t.display(self.sql));
            return Err(self.refuse(start, &text, "window definitions are not modelled"));
        }
        Ok(tables)
    }

    fn source_list(
        &mut self,
        ctes: &BTreeSet<String>,
        tables: &mut Tables,
    ) -> Result<(), Diagnostic> {
        loop {
            self.source_item(ctes, tables)?;
            if !self.eat_punct(",") {
                break;
            }
        }
        Ok(())
    }

    fn source_item(
        &mut self,
        ctes: &BTreeSet<String>,
        tables: &mut Tables,
    ) -> Result<(), Diagnostic> {
        self.source_base(ctes, tables)?;
        loop {
            let natural = self.eat_word("natural");
            let joined = self.at_word("join")
                || self.at_word("inner")
                || self.at_word("left")
                || self.at_word("right")
                || self.at_word("full")
                || self.at_word("cross");
            if !joined {
                if natural {
                    return Err(self.here("`natural` must be followed by a join"));
                }
                return Ok(());
            }
            self.eat_word("inner");
            if self.eat_word("left") || self.eat_word("right") || self.eat_word("full") {
                self.eat_word("outer");
            }
            self.eat_word("cross");
            self.expect_word("join")?;
            self.source_base(ctes, tables)?;
            if self.eat_word("on") {
                self.skip_expr(ctes, tables, CLAUSE_WORDS, true)?;
            } else if self.eat_word("using") {
                self.expect_punct("(")?;
                self.name_list()?;
            }
        }
    }

    fn source_base(
        &mut self,
        ctes: &BTreeSet<String>,
        tables: &mut Tables,
    ) -> Result<(), Diagnostic> {
        self.eat_word("lateral");
        if self.eat_punct("(") {
            if self.at_word("select") || self.at_word("values") || self.at_word("with") {
                let inner = self.statement_within(ctes)?;
                tables.absorb(inner.tables);
            } else {
                // A parenthesised join. Nothing new is reachable through it, but
                // it is a shape this parser does not model, and modelling half of
                // it is how a relation goes unseen.
                let start = self.tokens[self.at.saturating_sub(1)].start;
                return Err(self.refuse(
                    start,
                    "(",
                    "a parenthesised from-item is not a shape the scanner models",
                ));
            }
            self.expect_punct(")")?;
            self.alias()?;
            return Ok(());
        }
        self.eat_word("only");
        let name = self.qualified_name("a table name")?;
        // `f(x)` in a from position is a set-returning function, whose relations
        // are inside a function body no scanner can see.
        if self.peek().is_some_and(|t| t.is_punct("(")) {
            let start = self.peek().expect("checked").start;
            return Err(self.refuse(
                start,
                "(",
                "a set-returning function in `from` reads tables no scanner can name",
            ));
        }
        self.eat_punct("*");
        if !ctes.contains(&name) {
            tables.read(name);
        }
        self.alias()?;
        Ok(())
    }

    // --- insert / update / delete ---

    fn insert(&mut self, ctes: &BTreeSet<String>) -> Result<Scan, Diagnostic> {
        self.expect_word("insert")?;
        self.expect_word("into")?;
        let mut tables = Tables::default();
        let target = self.qualified_name("a table name")?;
        tables.write(target);
        self.alias()?;
        if self.eat_punct("(") {
            self.name_list()?;
        }
        if self.eat_word("default") {
            self.expect_word("values")?;
        } else {
            let inner = self.select_stmt(ctes)?;
            tables.absorb(inner);
        }
        if self.at_word("on") {
            let t = self.peek().expect("checked");
            let (start, text) = (t.start, t.display(self.sql));
            return Err(self.refuse(
                start,
                &text,
                "`on conflict` is not modelled: an upsert is a write whose outcome the engine cannot reproduce",
            ));
        }
        self.returning(ctes, &mut tables)?;
        Ok(Scan {
            kind: Kind::Insert,
            tables,
        })
    }

    fn update(&mut self, ctes: &BTreeSet<String>) -> Result<Scan, Diagnostic> {
        self.expect_word("update")?;
        self.eat_word("only");
        let mut tables = Tables::default();
        let target = self.qualified_name("a table name")?;
        tables.write(target);
        self.alias()?;
        self.expect_word("set")?;
        self.skip_expr(ctes, &mut tables, &["from", "where", "returning"], false)?;
        if self.eat_word("from") {
            self.source_list(ctes, &mut tables)?;
        }
        if self.eat_word("where") {
            self.skip_expr(ctes, &mut tables, &["returning"], false)?;
        }
        self.returning(ctes, &mut tables)?;
        Ok(Scan {
            kind: Kind::Update,
            tables,
        })
    }

    fn delete(&mut self, ctes: &BTreeSet<String>) -> Result<Scan, Diagnostic> {
        self.expect_word("delete")?;
        self.expect_word("from")?;
        self.eat_word("only");
        let mut tables = Tables::default();
        let target = self.qualified_name("a table name")?;
        tables.write(target);
        self.alias()?;
        if self.eat_word("using") {
            self.source_list(ctes, &mut tables)?;
        }
        if self.eat_word("where") {
            self.skip_expr(ctes, &mut tables, &["returning"], false)?;
        }
        self.returning(ctes, &mut tables)?;
        Ok(Scan {
            kind: Kind::Delete,
            tables,
        })
    }

    fn returning(
        &mut self,
        ctes: &BTreeSet<String>,
        tables: &mut Tables,
    ) -> Result<(), Diagnostic> {
        if self.eat_word("returning") {
            self.skip_expr(ctes, tables, &[], false)?;
        }
        Ok(())
    }

    // --- pieces ---

    /// The select list, one target at a time.
    ///
    /// `AS` is a clause boundary everywhere else in the grammar and is an output
    /// column name here, so the target list cannot be one `skip_expr`: walking it
    /// as one stops at the first alias, never eats the `from`, and comes back
    /// with an empty table set. That is the shape `derive row` needs whenever a
    /// statement joins two tables, and the shape `E0433`'s own advice — "alias
    /// one of them: `select a.id as a_id, b.id as b_id`" — tells a reader to
    /// write.
    fn target_list(
        &mut self,
        ctes: &BTreeSet<String>,
        tables: &mut Tables,
    ) -> Result<(), Diagnostic> {
        loop {
            self.skip_expr(ctes, tables, CLAUSE_WORDS, true)?;
            if self.eat_word("as") {
                // Any word: postgres reserves almost nothing after `AS`, and a
                // refusal here would be the scanner inventing a rule.
                self.identifier("an output column name")?;
            }
            if !self.eat_punct(",") {
                return Ok(());
            }
        }
    }

    /// Whether the word at `self.at` opens a function call, and whether that call
    /// is one the trusted computing base will run.
    ///
    /// This is the second half of §2.4. The first decides statement *shapes*; a
    /// call in an expression is inside every shape the scanner admits, and it is
    /// the only way an admitted statement reaches state the footprint cannot
    /// name — the connection's own session, which is the thing two host-backed
    /// tests share.
    fn check_call(&self) -> Result<(), Diagnostic> {
        let token = self.peek().expect("called with a token");
        if token.kind != TokenKind::Word || token.quoted {
            return Ok(());
        }
        if !self.peek_at(1).is_some_and(|t| t.is_punct("(")) {
            return Ok(());
        }
        let name = token.text.as_str();
        if is_reserved(name) || SYNTACTIC.contains(&name) || CALLABLE.contains(&name) {
            return Ok(());
        }
        let (start, text) = (token.start, token.display(self.sql));
        Err(self
            .refuse(
                start,
                &text,
                "this function is not one the scanner will vouch for",
            )
            .note("a call is the one place an admitted statement can reach outside its own table set: `set_config` rebinds the `search_path` every later borrower of the pooled connection inherits, `pg_advisory_lock` holds a session lock the next one blocks on, and both report the footprint of whatever relation they were selected `from`")
            .note(format!("the scanner calls: {}", CALLABLE.join(" "))))
    }

    /// Walk an expression, following any subquery inside it and stopping at the
    /// next clause keyword at depth zero.
    ///
    /// A subquery is followed rather than refused because following it keeps the
    /// table set a superset, and refusing `where id in (select …)` would make
    /// half the statements a service writes inexpressible for no gain.
    fn skip_expr(
        &mut self,
        ctes: &BTreeSet<String>,
        tables: &mut Tables,
        stops: &[&str],
        stop_at_comma: bool,
    ) -> Result<(), Diagnostic> {
        let mut depth = 0usize;
        loop {
            let Some(token) = self.peek() else {
                return Ok(());
            };
            if token.kind == TokenKind::Semicolon {
                let start = token.start;
                return Err(self.stacked(start));
            }
            if token.is_punct("(") {
                self.advance();
                if self.at_word("select") || self.at_word("values") || self.at_word("with") {
                    let inner = self.statement_within(ctes)?;
                    tables.absorb(inner.tables);
                    self.expect_punct(")")?;
                } else {
                    depth += 1;
                }
                continue;
            }
            if token.is_punct(")") {
                if depth == 0 {
                    return Ok(());
                }
                depth -= 1;
                self.advance();
                continue;
            }
            if depth == 0 {
                if stop_at_comma && token.is_punct(",") {
                    return Ok(());
                }
                if token.kind == TokenKind::Word
                    && !token.quoted
                    && stops.contains(&token.text.as_str())
                {
                    return Ok(());
                }
            }
            self.check_call()?;
            self.advance();
        }
    }

    /// The body of a parenthesised group whose opening paren is already eaten.
    fn skip_parens(
        &mut self,
        ctes: &BTreeSet<String>,
        tables: &mut Tables,
    ) -> Result<(), Diagnostic> {
        self.skip_expr(ctes, tables, &[], false)?;
        self.expect_punct(")")
    }

    fn name_list(&mut self) -> Result<Vec<String>, Diagnostic> {
        let mut out = Vec::new();
        loop {
            out.push(self.identifier("a column name")?);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;
        Ok(out)
    }

    /// `[schema.]name`, answering the **last** segment.
    ///
    /// A resource label in Ply is a ground identifier, so `public.items` and
    /// `items` are one resource: the schema qualifies where the relation lives
    /// and not what it is. Two relations of one name in two schemas would
    /// therefore share a label, which is stated in `ply hosts` rather than
    /// silently true — a `search_path` with two schemas is outside what W4
    /// models.
    fn qualified_name(&mut self, what: &str) -> Result<String, Diagnostic> {
        let mut name = self.identifier(what)?;
        while self.peek().is_some_and(|t| t.is_punct("."))
            && self.peek_at(1).is_some_and(|t| t.kind == TokenKind::Word)
        {
            self.advance();
            name = self.identifier(what)?;
        }
        Ok(name)
    }

    fn identifier(&mut self, what: &str) -> Result<String, Diagnostic> {
        match self.peek() {
            Some(t) if t.kind == TokenKind::Word => {
                let name = t.text.clone();
                self.advance();
                Ok(name)
            }
            Some(t) => {
                let (start, text) = (t.start, t.display(self.sql));
                Err(self.refuse(start, &text, &format!("{what} was expected here")))
            }
            None => Err(self.here(&format!("{what} was expected here"))),
        }
    }

    /// `[AS] alias [(columns)]`, if there is one.
    fn alias(&mut self) -> Result<(), Diagnostic> {
        if self.eat_word("as") {
            self.identifier("an alias")?;
        } else if self
            .peek()
            .is_some_and(|t| t.kind == TokenKind::Word && (t.quoted || !is_reserved(&t.text)))
        {
            self.advance();
        } else {
            return Ok(());
        }
        if self.eat_punct("(") {
            self.name_list()?;
        }
        Ok(())
    }

    fn expect_word(&mut self, word: &str) -> Result<(), Diagnostic> {
        if self.eat_word(word) {
            return Ok(());
        }
        match self.peek() {
            Some(t) => {
                let (start, text) = (t.start, t.display(self.sql));
                Err(self.refuse(start, &text, &format!("`{word}` was expected here")))
            }
            None => Err(self.here(&format!("`{word}` was expected here"))),
        }
    }

    fn reject_nondeterministic(&self) -> Result<(), Diagnostic> {
        for token in &self.tokens {
            if token.kind != TokenKind::Word || token.quoted {
                continue;
            }
            if !NONDETERMINISTIC.contains(&token.text.as_str()) {
                continue;
            }
            return Err(Diagnostic::error(
                codes::DB_STATEMENT_REFUSED,
                format!(
                    "`{}` is not a function of the program's state, and this statement calls it at byte {}",
                    token.text, token.start
                ),
            )
            .primary(self.span, "this statement reaches the database driver")
            .note("pass the value as a parameter instead — `clock.now()` bound to a `$1` puts the nondeterminism in the row, where `E0412` can see it")
            .note("hidden inside statement text it is invisible to the effect system, so a `det` test would read a different value on every run and nothing would say so"));
        }
        Ok(())
    }

    #[cold]
    fn stacked(&self, offset: usize) -> Diagnostic {
        Diagnostic::error(
            codes::DB_STATEMENT_REFUSED,
            format!("this `Stmt` holds more than one statement: a `;` at byte {offset}"),
        )
        .primary(self.span, "this statement reaches the database driver")
        .note("one `Stmt` is one statement, which is what removes the stacked-statement payload an injected fragment would otherwise become")
        .note("a `;` inside a string literal or a dollar-quoted body is ordinary text and is not this")
        .note("perform one `db.execute` per statement")
    }

    #[cold]
    fn here(&self, why: &str) -> Diagnostic {
        let offset = self
            .peek()
            .map(|t| t.start)
            .unwrap_or_else(|| self.sql.len());
        let text = self.peek().map(|t| t.display(self.sql)).unwrap_or_default();
        self.refuse(offset, &text, why)
    }

    #[cold]
    fn refuse(&self, offset: usize, token: &str, why: &str) -> Diagnostic {
        let named = if token.is_empty() {
            format!("the statement ends at byte {offset}")
        } else {
            format!("`{token}` at byte {offset}")
        };
        Diagnostic::error(
            codes::DB_STATEMENT_REFUSED,
            format!("the database driver refuses this statement: {named}"),
        )
        .primary(self.span, "this statement reaches the database driver")
        .note(why.to_string())
        .note(format!("the scanner accepts: {ACCEPTED}"))
        .note("it computes the table set an endpoint's row is checked against, so a construct it cannot account for is refused rather than run with a footprint that under-reports")
    }
}

#[cold]
fn unterminated(span: Span, offset: usize, what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::DB_STATEMENT_REFUSED,
        format!("{what} opened at byte {offset} is never closed"),
    )
    .primary(span, "this statement reaches the database driver")
    .note("the scanner cannot tell text from statement past an unterminated quote, so it refuses rather than guessing where the literal ended")
}

/// Words that cannot be an alias without `AS`.
///
/// Deliberately over-inclusive rather than postgres's exact reserved list: a
/// word wrongly treated as reserved makes the parser expect a clause and refuse,
/// which is loud, while a clause keyword wrongly taken as an alias would swallow
/// the clause and lose whatever relation it named.
fn is_reserved(word: &str) -> bool {
    CLAUSE_WORDS.contains(&word)
        || matches!(
            word,
            "select"
                | "insert"
                | "update"
                | "delete"
                | "with"
                | "into"
                | "and"
                | "or"
                | "not"
                | "null"
                | "is"
                | "in"
                | "between"
                | "like"
                | "ilike"
                | "similar"
                | "when"
                | "then"
                | "else"
                | "end"
                | "case"
                | "asc"
                | "desc"
                | "nulls"
                | "first"
                | "last"
                | "all"
                | "distinct"
                | "only"
                | "lateral"
                | "default"
                | "conflict"
                | "do"
                | "nothing"
                | "recursive"
                | "materialized"
                | "by"
                | "outer"
        )
}

#[cfg(test)]
mod tests;
