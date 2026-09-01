//! Errors are collected, not returned: a failed item is abandoned, the token stream is
//! resynchronised on the next item keyword, and parsing continues, so one run reports as many
//! independent syntax errors as it can find.

use crate::ast::*;
use crate::lexer::{Kw, Token, TokenKind, lex};
use ply_span::{Diagnostic, SourceId, Span, codes};

/// Signals that the enclosing construct was abandoned.
pub struct Bail;

/// What one comma-separated member of `{..}` turned out to be.
enum RowMember {
    Atom(AtomExpr),
    Set(QName),
}

/// What one comma-separated member of an argument list turned out to be.
enum Arg {
    Positional(Expr),
    Named(NamedArg),
}

type PResult<T> = Result<T, Bail>;

/// Recursive descent walks the Rust stack, so nesting has to be capped before it overflows.
const MAX_DEPTH: u32 = 128;

/// Parses a snippet as the anonymous module: it can neither import nor be imported, which is all a
/// test or an editor scratch buffer needs.
pub fn parse(source: SourceId, text: &str) -> Result<Module, Vec<Diagnostic>> {
    parse_module(source, ModuleName::anonymous(), text)
}

pub fn parse_module(
    source: SourceId,
    name: ModuleName,
    text: &str,
) -> Result<Module, Vec<Diagnostic>> {
    let (module, diags) = Parser::new(source, text).run(name);
    if diags.is_empty() {
        Ok(module)
    } else {
        Err(diags)
    }
}

/// Parses as much as possible and hands back both the partial tree and every diagnostic.
pub fn parse_recovering(
    source: SourceId,
    name: ModuleName,
    text: &str,
) -> (Module, Vec<Diagnostic>) {
    Parser::new(source, text).run(name)
}

/// **The tree before `effect_set`, `record_update` and `try_op` rewrite it.**
#[doc(hidden)]
pub fn parse_unexpanded(
    source: SourceId,
    name: ModuleName,
    text: &str,
) -> (Module, Vec<Diagnostic>) {
    Parser::new(source, text).run_unexpanded(name)
}

/// Each input becomes its own module.
pub fn parse_program<'a>(
    inputs: impl IntoIterator<Item = (SourceId, ModuleName, &'a str)>,
) -> Result<Program, Vec<Diagnostic>> {
    let mut program = Program::default();
    let mut diags = Vec::new();
    for (source, name, text) in inputs {
        let (module, d) = parse_recovering(source, name, text);
        program.modules.push(module);
        diags.extend(d);
    }
    if diags.is_empty() {
        Ok(program)
    } else {
        Err(diags)
    }
}

pub fn parse_expr(source: SourceId, text: &str) -> Result<Expr, Vec<Diagnostic>> {
    let mut p = Parser::new(source, text);
    match p.expr() {
        Ok(mut e) if p.at(&TokenKind::Eof) && p.diags.is_empty() => {
            // A bare expression has no module around it, so a record update here has no shape to
            // resolve and every one refuses.
            if p.uses_record_update {
                crate::record_update::expand_bare(&mut e, &mut p.diags);
            }
            // Every `?` here refuses: a bare expression has no enclosing `fn`, so there is no
            // written return type to read `Ok`/`Err` off.
            if p.uses_try {
                crate::try_op::expand_bare(&mut e, &mut p.diags);
            }
            match p.diags.is_empty() {
                true => Ok(e),
                false => Err(p.diags),
            }
        }
        Ok(_) => {
            p.error_here("end of input after the expression");
            Err(p.diags)
        }
        Err(Bail) => Err(p.diags),
    }
}

struct Parser {
    source: SourceId,
    tokens: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
    /// Inside an `if`/`match` scrutinee a `{` starts the arm block, never a record or block
    /// expression.
    no_brace: bool,
    /// Inside a lambda's parameter list, where a `|` closes the list rather than being bit-or.
    no_pipe: bool,
    depth: u32,
    /// Whether the file declared an `effect set` or named one in a row.
    uses_effect_sets: bool,
    /// Whether the file wrote `{..b, ...}` anywhere.
    uses_record_update: bool,
    /// Whether the file wrote a `?` anywhere.
    uses_try: bool,
}

impl Parser {
    fn new(source: SourceId, text: &str) -> Parser {
        let (tokens, diags) = lex(source, text);
        Parser {
            source,
            tokens,
            pos: 0,
            diags,
            no_brace: false,
            no_pipe: false,
            depth: 0,
            uses_effect_sets: false,
            uses_record_update: false,
            uses_try: false,
        }
    }

    fn kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn kind_at(&self, n: usize) -> &TokenKind {
        let i = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[i].kind
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    /// Whether the `n` tokens after the cursor repeat the one at it with nothing between them,
    /// which is the whole difference between `>>` and the error `a > > b` has always been.
    fn joined(&self, n: usize) -> bool {
        let here = &self.tokens[self.pos];
        (1..=n).all(|i| match self.tokens.get(self.pos + i) {
            Some(t) => {
                t.kind == here.kind
                    && t.span.source == here.span.source
                    && t.span.start == self.tokens[self.pos + i - 1].span.end
            }
            None => false,
        })
    }

    fn advance(&mut self) -> Span {
        let s = self.span();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        s
    }

    fn at(&self, k: &TokenKind) -> bool {
        self.kind() == k
    }

    fn at_eof(&self) -> bool {
        matches!(self.kind(), TokenKind::Eof)
    }

    /// The token `n` ahead, clamped at the end.
    fn peek_is(&self, n: usize, k: &TokenKind) -> bool {
        let i = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[i].kind == k
    }

    fn at_ident_text(&self, text: &str) -> bool {
        matches!(self.kind(), TokenKind::Ident(n) if n.as_str() == text)
    }

    fn eat(&mut self, k: &TokenKind) -> bool {
        if self.at(k) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, k: &TokenKind, what: &str) -> PResult<Span> {
        if self.at(k) {
            Ok(self.advance())
        } else {
            Err(self.error_here(what))
        }
    }

    fn expect_close(&mut self, k: &TokenKind, open: Span, what: &str) -> PResult<Span> {
        if self.at(k) {
            Ok(self.advance())
        } else {
            Err(self.unclosed(open, what))
        }
    }

    fn unclosed(&mut self, open: Span, what: &str) -> Bail {
        let found = self.kind().describe();
        let span = self.span();
        self.push(
            Diagnostic::error(
                codes::UNEXPECTED_TOKEN,
                format!("expected {what}, found {found}"),
            )
            .primary(span, format!("expected {what}"))
            .secondary(open, "unclosed delimiter opened here"),
        );
        Bail
    }

    fn expect_ident(&mut self, what: &str) -> PResult<Ident> {
        if let TokenKind::Ident(n) = self.kind() {
            let name = n.clone();
            let span = self.advance();
            Ok(Ident::new(name, span))
        } else {
            Err(self.error_here(what))
        }
    }

    /// `>=` immediately after a type parameter list is really `>` then `=`, as in `type Pair<a>=
    /// ..`
    fn expect_gt(&mut self, what: &str) -> PResult<Span> {
        match self.kind() {
            TokenKind::Gt => Ok(self.advance()),
            TokenKind::Ge => {
                let span = self.span();
                let gt = Span::new(span.source, span.start, span.start + 1);
                self.tokens[self.pos] = Token {
                    kind: TokenKind::Eq,
                    span: Span::new(span.source, span.start + 1, span.end),
                };
                Ok(gt)
            }
            _ => Err(self.error_here(what)),
        }
    }

    fn push(&mut self, d: Diagnostic) {
        // A single mistake usually trips several expectations at the same offset; only the first is
        // informative.
        if let Some(last) = self.diags.last()
            && last.code == d.code
            && last.primary_span() == d.primary_span()
        {
            return;
        }
        self.diags.push(d);
    }

    /// Out of line, and that is not a style preference.
    #[cold]
    #[inline(never)]
    fn no_named_arguments_on_a_perform(&mut self, effect: &QName, op: &Ident, named: &[NamedArg]) {
        for n in named {
            self.push(
                Diagnostic::error(
                    codes::UNKNOWN_ARGUMENT_NAME,
                    format!("`{}.{}` takes no named arguments", effect, op.name),
                )
                .primary(n.span, "named")
                .note("an effect operation's arguments are positional"),
            );
        }
    }

    #[cold]
    #[inline(never)]
    fn positional_after_named(&mut self, at: Span, first: Span) {
        self.push(
            Diagnostic::error(
                codes::ARGUMENT_ORDER,
                "a positional argument cannot follow a named one",
            )
            .primary(at, "positional")
            .secondary(first, "the first named argument is here")
            .note(
                "positional arguments fill parameters left to right, which a name in front \
                 of them would make ambiguous",
            ),
        );
    }

    fn error_here(&mut self, what: &str) -> Bail {
        let found = self.kind().describe();
        let span = self.span();
        self.push(
            Diagnostic::error(
                codes::UNEXPECTED_TOKEN,
                format!("expected {what}, found {found}"),
            )
            .primary(span, format!("expected {what}")),
        );
        Bail
    }

    fn deeper(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth <= MAX_DEPTH {
            return Ok(());
        }
        let span = self.span();
        self.push(
            Diagnostic::error(
                codes::UNEXPECTED_TOKEN,
                "input is nested too deeply to parse",
            )
            .primary(span, format!("more than {MAX_DEPTH} levels of nesting"))
            .note("split this into smaller definitions"),
        );
        Err(Bail)
    }

    fn run(mut self, name: ModuleName) -> (Module, Vec<Diagnostic>) {
        let mut module = self.parse_all(name);
        if self.uses_effect_sets {
            crate::effect_set::expand(&mut module, &mut self.diags);
        }
        if self.uses_record_update {
            crate::record_update::expand(&mut module, &mut self.diags);
        }
        // Last, by convention rather than by necessity.
        if self.uses_try {
            crate::try_op::expand(&mut module, &mut self.diags);
        }
        (module, self.diags)
    }

    /// [`run`](Self::run) with the three rewrites above **not** run: the tree exactly as the
    /// grammar built it, `ExprKind::Try` and `ExprKind::RecordUpdate` still in it and every effect
    /// row still holding only the atoms that were written.
    fn run_unexpanded(mut self, name: ModuleName) -> (Module, Vec<Diagnostic>) {
        let module = self.parse_all(name);
        (module, self.diags)
    }

    /// The grammar and the recovery loop, with no rewrite after them.
    fn parse_all(&mut self, name: ModuleName) -> Module {
        let source = self.source;
        let mut imports = self.imports();
        let mut items = Vec::new();
        while !self.at_eof() {
            self.no_brace = false;
            self.depth = 0;
            if self.at(&TokenKind::Kw(Kw::Import)) {
                self.import_out_of_order(items.first());
                match self.import_decl() {
                    Ok(decl) => imports.push(decl),
                    Err(Bail) => self.recover_to_item(),
                }
                continue;
            }
            match self.item() {
                Ok(item) => items.push(item),
                Err(Bail) => self.recover_to_item(),
            }
        }
        Module {
            name,
            source,
            imports,
            items,
        }
    }

    /// Every `import` precedes every item, so the import table is complete before any body is
    /// parsed.
    fn imports(&mut self) -> Vec<ImportDecl> {
        let mut out = Vec::new();
        while self.at(&TokenKind::Kw(Kw::Import)) {
            match self.import_decl() {
                Ok(decl) => out.push(decl),
                Err(Bail) => self.recover_to_item(),
            }
        }
        out
    }

    /// Reported rather than silently accepted: a later pass would otherwise have to look ahead to
    /// know what a name in an earlier body could mean.
    fn import_out_of_order(&mut self, first_item: Option<&Item>) {
        let span = self.span();
        let mut d = Diagnostic::error(
            codes::UNEXPECTED_TOKEN,
            "`import` must appear before every definition",
        )
        .primary(span, "this `import` follows a definition")
        .note("move it to the top of the file, above the first `fn`, `type`, `effect` or `test`");
        if let Some(item) = first_item {
            d = d.secondary(item.span(), "the first definition is here");
        }
        self.push(d);
    }

    fn import_decl(&mut self) -> PResult<ImportDecl> {
        let start = self.advance();
        let mut path = vec![self.expect_ident("a module path after `import`")?];
        while self.eat(&TokenKind::Dot) {
            path.push(self.expect_ident("another module path segment after `.`")?);
        }

        // `as` is contextual: it stays usable as an ordinary identifier.
        let kind = if self.at_ident_text("as") {
            self.advance();
            ImportKind::Alias(self.expect_ident("a name to bind the module as, after `as`")?)
        } else if self.at(&TokenKind::LParen) {
            let open = self.advance();
            let names = self.comma_list(&TokenKind::RParen, Self::import_name)?;
            let close =
                self.expect_close(&TokenKind::RParen, open, "`)` to close the imported names")?;
            if names.is_empty() {
                self.push(
                    Diagnostic::error(codes::UNEXPECTED_TOKEN, "this `import` selects no names")
                        .primary(open.to(close), "an empty list imports nothing")
                        .note(
                            "write `import <module>` to bind the module itself, \
                             or list the names to bring into scope",
                        ),
                );
                return Err(Bail);
            }
            ImportKind::Names(names)
        } else {
            ImportKind::Module
        };

        let both = match &kind {
            ImportKind::Alias(_) => self.at(&TokenKind::LParen),
            ImportKind::Names(_) => self.at_ident_text("as"),
            ImportKind::Module => false,
        };
        if both {
            let span = self.span();
            self.push(
                Diagnostic::error(
                    codes::UNEXPECTED_TOKEN,
                    "an `import` may rename the module or select names, but not both",
                )
                .primary(span, "remove this")
                .note(
                    "`import m as x` binds the module as `x`; \
                     `import m (a, b)` binds `a` and `b` and no module binder",
                ),
            );
            return Err(Bail);
        }

        Ok(ImportDecl {
            path,
            kind,
            span: start.to(self.prev_span()),
        })
    }

    fn import_name(&mut self) -> PResult<Ident> {
        self.expect_ident("a name to import from the module")
    }

    fn at_item_start(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Kw(
                Kw::Import | Kw::Pub | Kw::Fn | Kw::Type | Kw::Effect | Kw::Nondet | Kw::Test
            )
        ) || self.at_law_start()
            || self.at_derive_start()
    }

    /// `law` is contextual: it opens an item only when a quoted label follows, so `fn law(..)` and
    /// a local named `law` keep their meaning.
    fn at_law_start(&self) -> bool {
        self.at_ident_text("law")
            && (matches!(self.kind_at(1), TokenKind::Str(_))
                || (matches!(self.kind_at(1), TokenKind::Slash)
                    && matches!(self.kind_at(2), TokenKind::Ident(_))))
    }

    /// `derive` is contextual for the same reason `law` is.
    fn at_derive_start(&self) -> bool {
        self.at_ident_text("derive") && matches!(self.kind_at(1), TokenKind::Ident(_))
    }

    fn recover_to_item(&mut self) {
        let mut depth = 0i32;
        // Both callers consume the leading keyword before they can fail, so already being at an
        // item start means progress was made and the token belongs to the next construct rather
        // than the abandoned one.
        if !self.at_eof() && !self.at_item_start() {
            self.advance();
        }
        loop {
            if self.at_eof() {
                return;
            }
            if depth == 0 && self.at_item_start() {
                return;
            }
            match self.kind() {
                TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket => {
                    depth = (depth - 1).max(0)
                }
                _ => {}
            }
            self.advance();
        }
    }

    /// A qualified reference uses `::` rather than `.` because with `.` it would be token-identical
    /// to a perform and to a field access, and telling those apart needs scope information the
    /// parser does not have.
    fn qname(&mut self, what: &str) -> PResult<QName> {
        let first = self.expect_ident(what)?;
        if !self.eat(&TokenKind::ColonColon) {
            return Ok(QName::bare(first));
        }
        let name = self.expect_ident("a name after `::`")?;
        if self.at(&TokenKind::ColonColon) {
            let span = self.span();
            self.push(
                Diagnostic::error(
                    codes::UNEXPECTED_TOKEN,
                    "a qualified name has at most one `::`",
                )
                .primary(span, "unexpected `::`")
                .note(
                    "a module binder is a single name: `import store.orders` binds `orders`, \
                         so write `orders::place`",
                ),
            );
            return Err(Bail);
        }
        Ok(QName::qualified(first, name))
    }

    fn item(&mut self) -> PResult<Item> {
        let pub_span = self.at(&TokenKind::Kw(Kw::Pub)).then(|| self.advance());
        let vis = if pub_span.is_some() {
            Visibility::Public
        } else {
            Visibility::Private
        };

        match self.kind() {
            TokenKind::Kw(Kw::Fn) => self.fn_def(vis).map(|d| Item::Fn(Box::new(d))),
            TokenKind::Kw(Kw::Type) => self.type_def(vis).map(|d| Item::Type(Box::new(d))),
            _ if self.at_effect_set_start() => {
                if let Some(span) = pub_span {
                    self.push(
                        Diagnostic::error(
                            codes::UNKNOWN_EFFECT_SET,
                            "an `effect set` cannot be `pub`",
                        )
                        .primary(span, "remove `pub`")
                        .note(
                            "an `effect set` may only be used in the module that declares it, so \
                             there is nothing for `pub` to publish",
                        )
                        .note(
                            "a set expanding across a module boundary would let an edit in the \
                             declaring module leave a stale published row behind in a file whose \
                             bytes never moved",
                        ),
                    );
                }
                self.effect_set_def().map(|d| Item::EffectSet(Box::new(d)))
            }
            TokenKind::Kw(Kw::Effect) | TokenKind::Kw(Kw::Nondet) => {
                self.effect_def(vis).map(|d| Item::Effect(Box::new(d)))
            }
            TokenKind::Kw(Kw::Test) => {
                if let Some(span) = pub_span {
                    self.push(
                        Diagnostic::error(codes::UNEXPECTED_TOKEN, "a `test` cannot be `pub`")
                            .primary(span, "remove `pub`")
                            .note("a test has no name another module could reference"),
                    );
                }
                self.test_def().map(|d| Item::Test(Box::new(d)))
            }
            _ if self.at_law_start() => {
                if let Some(span) = pub_span {
                    self.push(
                        Diagnostic::error(codes::UNEXPECTED_TOKEN, "a `law` cannot be `pub`")
                            .primary(span, "remove `pub`")
                            .note("a law has no name another module could reference"),
                    );
                }
                self.law_def().map(|d| Item::Law(Box::new(d)))
            }
            _ if self.at_derive_start() => {
                if let Some(span) = pub_span {
                    self.push(
                        Diagnostic::error(codes::UNEXPECTED_TOKEN, "a `derive` cannot be `pub`")
                            .primary(span, "remove `pub`")
                            .note(
                                "a generated definition takes the visibility of the type it is \
                                 derived for, so a type you can name is a type you can encode",
                            ),
                    );
                }
                self.derive_def().map(|d| Item::Derive(Box::new(d)))
            }
            _ => Err(self.error_here(
                "an item: `fn`, `type`, `effect`, `nondet effect`, `effect set`, `test`, `law`, \
                 or `derive`",
            )),
        }
    }

    /// `effect set Web = {..}`.
    fn at_effect_set_start(&self) -> bool {
        self.at(&TokenKind::Kw(Kw::Effect))
            && matches!(self.kind_at(1), TokenKind::Ident(n) if n.as_str() == "set")
            && matches!(self.kind_at(2), TokenKind::Ident(_))
    }

    fn effect_set_def(&mut self) -> PResult<EffectSetDef> {
        self.uses_effect_sets = true;
        let start = self.advance();
        self.advance();
        let name = self.expect_ident("a name for the effect set, after `effect set`")?;
        self.expect(&TokenKind::Eq, "`=` after the effect set's name")?;
        let open = self.expect(&TokenKind::LBrace, "`{` to open the effect set's members")?;

        let mut atoms = Vec::new();
        let mut includes = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Pipe) {
            if self.at_eof() {
                return Err(self.unclosed(open, "`}` to close the effect set"));
            }
            match self.row_member("an effect atom, or the name of another `effect set`")? {
                RowMember::Atom(a) => atoms.push(a),
                RowMember::Set(q) => includes.push(q),
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        // A set denotes a ground set of atoms, so there is no tail to abstract over and `| e` here
        // would have no meaning to give.
        if self.at(&TokenKind::Pipe) {
            let span = self.span();
            self.push(
                Diagnostic::error(
                    codes::UNEXPECTED_TOKEN,
                    "an `effect set` cannot carry a row variable",
                )
                .primary(span, "remove `| ..`")
                .note(
                    "a set abbreviates a fixed list of atoms; write the variable at the row that \
                     names the set",
                ),
            );
            return Err(Bail);
        }
        let close = self.expect_close(&TokenKind::RBrace, open, "`}` to close the effect set")?;

        Ok(EffectSetDef {
            name,
            atoms,
            includes,
            // Filled by `effect_set::expand`, which needs every set in the file before it can
            // resolve one.
            expansion: Vec::new(),
            span: start.to(close),
        })
    }

    /// `derive json for Order`.
    fn derive_def(&mut self) -> PResult<DeriveDef> {
        let start = self.advance();
        let name = self.expect_ident("a deriver name after `derive`")?;
        let deriver = self.deriver(&name)?;
        if !self.at_ident_text("for") {
            return Err(self.error_here("`for` and the type to derive for"));
        }
        self.advance();
        let target = self.expect_ident("the type to derive for, after `for`")?;
        Ok(DeriveDef {
            deriver,
            deriver_span: name.span,
            target,
            span: start.to(self.prev_span()),
        })
    }

    /// The derivers are fixed — there are no user-defined ones — so an unrecognized name is
    /// reported here with the whole list rather than left to fail as an unknown reference in
    /// generated code the user never wrote.
    fn deriver(&mut self, name: &Ident) -> PResult<Deriver> {
        match Deriver::from_name(name.name.as_str()) {
            Some(d) => Ok(d),
            None => {
                let all: Vec<String> = Deriver::ALL.iter().map(|d| format!("`{d}`")).collect();
                self.push(
                    Diagnostic::error(
                        codes::UNKNOWN_DERIVER,
                        format!("`{}` is not a deriver", name.name),
                    )
                    .primary(name.span, "unknown deriver")
                    .note(format!("the derivers are {}", all.join(", "))),
                );
                Err(Bail)
            }
        }
    }

    /// `where derivable(json, a), derivable(ord, k)`, between the effect row and any `requires`.
    fn where_clause(&mut self) -> PResult<Vec<Constraint>> {
        if !self.at_ident_text("where") {
            return Ok(Vec::new());
        }
        self.advance();
        let mut out = Vec::new();
        loop {
            out.push(self.constraint()?);
            if !self.eat(&TokenKind::Comma) {
                return Ok(out);
            }
        }
    }

    fn constraint(&mut self) -> PResult<Constraint> {
        let start = self.span();
        if !self.at_ident_text("derivable") {
            return Err(self.error_here("`derivable(<deriver>, <type parameter>)`"));
        }
        self.advance();
        let open = self.expect(&TokenKind::LParen, "`(` after `derivable`")?;
        let name = self.expect_ident("a deriver name")?;
        let deriver = self.deriver(&name)?;
        self.expect(&TokenKind::Comma, "`,` and the type parameter")?;
        let param = self.expect_ident("the type parameter the constraint is about")?;
        let close = self.expect_close(&TokenKind::RParen, open, "`)` to close `derivable`")?;
        Ok(Constraint {
            deriver,
            deriver_span: name.span,
            param,
            span: start.to(close),
        })
    }

    fn fn_def(&mut self, vis: Visibility) -> PResult<FnDef> {
        let start = self.advance();
        let name = self.expect_ident("a function name after `fn`")?;
        let generics = if self.at(&TokenKind::Lt) {
            self.generics()?
        } else {
            Generics::default()
        };

        let open = self.expect(&TokenKind::LParen, "`(` to start the parameter list")?;
        let params = self.comma_list(&TokenKind::RParen, Self::fn_param)?;
        self.expect_close(&TokenKind::RParen, open, "`)` to close the parameter list")?;

        let ret = if self.eat(&TokenKind::Arrow) {
            Some(self.ty()?)
        } else {
            None
        };
        let effects = if self.eat(&TokenKind::Slash) {
            Some(self.row()?)
        } else {
            None
        };

        let constraints = self.where_clause()?;
        let spec = self.spec_clauses()?;

        let body = if self.eat(&TokenKind::Eq) {
            self.expr()?
        } else if self.at(&TokenKind::LBrace) {
            self.block_expr()?
        } else {
            return Err(self.error_here("`=` or `{` to start the function body"));
        };

        let span = start.to(body.span);
        Ok(FnDef {
            vis,
            name,
            generics,
            params,
            ret,
            effects,
            constraints,
            derived: None,
            spec,
            body,
            span,
        })
    }

    /// `requires` and `ensures` are contextual, recognized only between a `fn` header and its body
    /// — where the grammar previously admitted nothing but `=` and `{`, so no ordinary name loses
    /// its meaning.
    fn spec_clauses(&mut self) -> PResult<Vec<SpecClause>> {
        let mut out = Vec::new();
        loop {
            let kind = if self.at_ident_text("requires") {
                SpecKind::Requires
            } else if self.at_ident_text("ensures") {
                SpecKind::Ensures
            } else {
                return Ok(out);
            };
            let start = self.advance();
            // Parsed like an `if` condition: a `{` closes the clause and opens the function's block
            // body, so `ensures p(x) { .. }` is a clause plus a body and never a record literal.
            let expr = self.scrutinee()?;
            let span = start.to(expr.span);
            out.push(SpecClause { kind, expr, span });
        }
    }

    fn law_def(&mut self) -> PResult<LawDef> {
        let start = self.advance();
        let mut host = false;
        if self.eat(&TokenKind::Slash) {
            if !self.at_ident_text("host") {
                return Err(self.error_here("`host` after `law/`"));
            }
            self.advance();
            host = true;
        }
        let (name, name_span) = match self.kind() {
            TokenKind::Str(s) => {
                let s = s.clone();
                (s, self.advance())
            }
            _ => return Err(self.error_here("a quoted law label")),
        };

        let mut binders = Vec::new();
        if self.at_ident_text("forall") {
            self.advance();
            let open = self.expect(&TokenKind::LParen, "`(` to start the `forall` binders")?;
            binders = self.comma_list(&TokenKind::RParen, Self::binder)?;
            let close = self.expect_close(&TokenKind::RParen, open, "`)` to close the binders")?;
            if binders.is_empty() {
                self.push(
                    Diagnostic::error(codes::UNEXPECTED_TOKEN, "this `forall` binds nothing")
                        .primary(open.to(close), "no binders")
                        .note("drop the `forall`: a law with no binders is a ground claim"),
                );
                return Err(Bail);
            }
        }

        let guard = if self.at_ident_text("where") {
            self.advance();
            Some(self.scrutinee()?)
        } else {
            None
        };

        let body = self.block_expr()?;
        let span = start.to(body.span);
        Ok(LawDef {
            name,
            name_span,
            host,
            binders,
            guard,
            body,
            span,
        })
    }

    /// A binder's type is mandatory: inferring it would make a law's meaning depend on how its body
    /// happened to be written.
    fn binder(&mut self) -> PResult<Binder> {
        let name = self.expect_ident("a binder name")?;
        self.expect(
            &TokenKind::Colon,
            "`:` and a type — a `forall` binder must be annotated",
        )?;
        let ty = self.ty()?;
        let span = name.span.to(ty.span());
        Ok(Binder { name, ty, span })
    }

    /// A `fn` parameter, which may carry `= <expr>`.
    fn fn_param(&mut self) -> PResult<Param> {
        self.param_inner(true)
    }

    /// A lambda parameter, which may not.
    fn param(&mut self) -> PResult<Param> {
        self.param_inner(false)
    }

    fn param_inner(&mut self, allow_default: bool) -> PResult<Param> {
        let name = self.expect_ident("a parameter name")?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.ty()?)
        } else {
            None
        };
        // Parsed either way, so that a default written on a lambda is refused with the reason
        // rather than with `expected `,` or `|``.
        let default = if self.at(&TokenKind::Eq) {
            let eq = self.advance();
            let e = self.expr()?;
            if allow_default {
                Some(e)
            } else {
                self.push(
                    Diagnostic::error(
                        codes::DEFAULT_NOT_ALLOWED,
                        "a lambda parameter cannot have a default",
                    )
                    .primary(eq.to(e.span), "no call can omit this argument")
                    .note(
                        "a default is filled in by matching a call against a signature, and a \
                         lambda is called through a value rather than by name",
                    ),
                );
                None
            }
        } else {
            None
        };
        let end = default
            .as_ref()
            .map(|d| d.span)
            .or_else(|| ty.as_ref().map(|t| t.span()))
            .unwrap_or(name.span);
        let span = name.span.to(end);
        Ok(Param {
            name,
            ty,
            default,
            span,
        })
    }

    fn generics(&mut self) -> PResult<Generics> {
        self.expect(&TokenKind::Lt, "`<`")?;
        let mut types = Vec::new();
        while !self.at(&TokenKind::Gt) && !self.at(&TokenKind::Ge) && !self.at(&TokenKind::Pipe) {
            if self.at_eof() {
                return Err(self.error_here("`>` to close the type parameter list"));
            }
            types.push(self.expect_ident("a type parameter name")?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let mut effects = Vec::new();
        if self.eat(&TokenKind::Pipe) {
            while !self.at(&TokenKind::Gt) && !self.at(&TokenKind::Ge) {
                if self.at_eof() {
                    return Err(self.error_here("`>` to close the type parameter list"));
                }
                effects.push(self.expect_ident("an effect parameter name")?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect_gt("`>` to close the type parameter list")?;
        Ok(Generics { types, effects })
    }

    fn type_def(&mut self, vis: Visibility) -> PResult<TypeDef> {
        let start = self.advance();
        let name = self.expect_ident("a type name after `type`")?;
        let mut params = Vec::new();
        if self.at(&TokenKind::Lt) {
            self.advance();
            while !self.at(&TokenKind::Gt) && !self.at(&TokenKind::Ge) {
                if self.at_eof() {
                    return Err(self.error_here("`>` to close the type parameter list"));
                }
                params.push(self.expect_ident("a type parameter name")?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect_gt("`>` to close the type parameter list")?;
        }
        self.expect(&TokenKind::Eq, "`=` after the type name")?;

        let body = if self.looks_like_variants() {
            TypeDefBody::Sum(self.variants()?)
        } else {
            TypeDefBody::Alias(self.ty()?)
        };
        let span = start.to(self.prev_span());
        Ok(TypeDef {
            vis,
            name,
            params,
            body,
            span,
        })
    }

    /// `type T = A` is an alias; a sum needs either a leading `|`, a payload, or a second variant,
    /// so that `type Id = Int` keeps meaning what it looks like.
    fn looks_like_variants(&self) -> bool {
        match self.kind() {
            TokenKind::Pipe => true,
            TokenKind::Ident(n) if starts_upper(n) => {
                matches!(self.kind_at(1), TokenKind::LParen | TokenKind::Pipe)
            }
            _ => false,
        }
    }

    fn variants(&mut self) -> PResult<Vec<VariantDef>> {
        self.eat(&TokenKind::Pipe);
        let mut out = Vec::new();
        loop {
            let name = self.expect_ident("a variant name")?;
            let mut fields = Vec::new();
            if self.at(&TokenKind::LParen) {
                let open = self.advance();
                fields = self.comma_list(&TokenKind::RParen, Self::ty)?;
                self.expect_close(&TokenKind::RParen, open, "`)` to close the variant fields")?;
            }
            let span = name.span.to(self.prev_span());
            out.push(VariantDef { name, fields, span });
            if !self.eat(&TokenKind::Pipe) {
                break;
            }
        }
        Ok(out)
    }

    fn effect_def(&mut self, vis: Visibility) -> PResult<EffectDef> {
        let start = self.span();
        let nondet = self.eat(&TokenKind::Kw(Kw::Nondet));
        self.expect(&TokenKind::Kw(Kw::Effect), "`effect` after `nondet`")?;
        let name = self.expect_ident("an effect name after `effect`")?;
        let open = self.expect(&TokenKind::LBrace, "`{` to start the operation list")?;
        let mut ops = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            ops.push(self.op_def()?);
        }
        let close =
            self.expect_close(&TokenKind::RBrace, open, "`}` to close the operation list")?;
        Ok(EffectDef {
            vis,
            name,
            nondet,
            ops,
            span: start.to(close),
        })
    }

    fn op_def(&mut self) -> PResult<OpDef> {
        let start = self.span();
        let mode = self.mode()?;
        let name = self.expect_ident("an operation name")?;
        let resource_param = if self.at(&TokenKind::LBracket) {
            let open = self.advance();
            self.expect_ident("a resource parameter name inside `[..]`")?;
            self.expect_close(&TokenKind::RBracket, open, "`]`")?;
            true
        } else {
            false
        };
        let open = self.expect(&TokenKind::LParen, "`(` to start the operation parameters")?;
        let params = self.comma_list(&TokenKind::RParen, Self::op_param)?;
        self.expect_close(
            &TokenKind::RParen,
            open,
            "`)` to close the operation parameters",
        )?;
        self.expect(&TokenKind::Arrow, "`->` and a return type")?;
        let ret = self.ty()?;
        let span = start.to(ret.span());
        Ok(OpDef {
            name,
            mode,
            resource_param,
            params,
            ret,
            span,
        })
    }

    /// An operation parameter may be written `name: Type` for documentation; only the type is part
    /// of the signature.
    fn op_param(&mut self) -> PResult<TypeExpr> {
        if matches!(self.kind(), TokenKind::Ident(_)) && matches!(self.kind_at(1), TokenKind::Colon)
        {
            self.advance();
            self.advance();
        }
        self.ty()
    }

    fn mode(&mut self) -> PResult<Mode> {
        if self.at_ident_text("read") {
            self.advance();
            Ok(Mode::Read)
        } else if self.at_ident_text("write") {
            self.advance();
            Ok(Mode::Write)
        } else {
            Err(self.error_here("`read` or `write` to start an operation"))
        }
    }

    fn test_def(&mut self) -> PResult<TestDef> {
        let start = self.advance();
        let mut nondet = false;
        if self.eat(&TokenKind::Slash) {
            self.expect(&TokenKind::Kw(Kw::Nondet), "`nondet` after `test/`")?;
            nondet = true;
        }
        let (name, name_span) = match self.kind() {
            TokenKind::Str(s) => {
                let s = s.clone();
                (s, self.advance())
            }
            _ => return Err(self.error_here("a quoted test name")),
        };
        let body = self.block_expr()?;
        let span = start.to(body.span);
        Ok(TestDef {
            name,
            name_span,
            nondet,
            body,
            span,
        })
    }

    fn ty(&mut self) -> PResult<TypeExpr> {
        let saved = std::mem::replace(&mut self.no_brace, false);
        let r = self.ty_inner();
        self.no_brace = saved;
        r
    }

    fn ty_inner(&mut self) -> PResult<TypeExpr> {
        self.deeper()?;
        let r = self.ty_body();
        self.depth -= 1;
        r
    }

    fn ty_body(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        match self.kind() {
            TokenKind::LParen => {
                let open = self.advance();
                if self.at(&TokenKind::RParen) {
                    let close = self.advance();
                    if self.eat(&TokenKind::Arrow) {
                        return self.fn_ty(Vec::new(), start);
                    }
                    return Ok(TypeExpr::Unit {
                        span: open.to(close),
                    });
                }
                let params = self.comma_list(&TokenKind::RParen, Self::ty)?;
                let close = self.expect_close(&TokenKind::RParen, open, "`)`")?;
                if self.eat(&TokenKind::Arrow) {
                    return self.fn_ty(params, start);
                }
                if params.len() == 1 {
                    return Ok(params.into_iter().next().expect("length checked"));
                }
                let span = open.to(close);
                self.push(
                    Diagnostic::error(
                        codes::UNEXPECTED_TOKEN,
                        "expected `->` after a parenthesized parameter list",
                    )
                    .primary(span, "this is a function parameter list")
                    .note("Ply has no tuple type; write a record `{a: A, b: B}` instead"),
                );
                Err(Bail)
            }
            TokenKind::LBrace => {
                let open = self.advance();
                let fields = self.comma_list(&TokenKind::RBrace, Self::ty_field)?;
                let close = self.expect_close(&TokenKind::RBrace, open, "`}`")?;
                Ok(TypeExpr::Record {
                    fields,
                    span: open.to(close),
                })
            }
            TokenKind::Ident(_) => {
                let q = self.qname("a type")?;
                if self.at(&TokenKind::Lt) {
                    self.advance();
                    let args = self.comma_list(&TokenKind::Gt, Self::ty)?;
                    let close = self.expect_gt("`>` to close the type arguments")?;
                    return Ok(TypeExpr::Con {
                        name: q,
                        args,
                        span: start.to(close),
                    });
                }
                // A type parameter is bound by the enclosing `<..>`, never by a module, so only a
                // bare lowercase name can be one.
                if q.is_bare() && !starts_upper(q.symbol()) {
                    return Ok(TypeExpr::Var(q.name));
                }
                let span = q.span;
                Ok(TypeExpr::Con {
                    name: q,
                    args: Vec::new(),
                    span,
                })
            }
            _ => Err(self.error_here("a type")),
        }
    }

    fn fn_ty(&mut self, params: Vec<TypeExpr>, start: Span) -> PResult<TypeExpr> {
        let ret = self.ty()?;
        let effects = if self.eat(&TokenKind::Slash) {
            Some(self.row()?)
        } else {
            None
        };
        let end = effects.as_ref().map_or(ret.span(), |r| r.span);
        Ok(TypeExpr::Fn {
            params,
            ret: Box::new(ret),
            effects,
            span: start.to(end),
        })
    }

    fn ty_field(&mut self) -> PResult<(Ident, TypeExpr)> {
        let name = self.expect_ident("a field name")?;
        self.expect(&TokenKind::Colon, "`:` after the field name")?;
        Ok((name, self.ty()?))
    }

    fn row(&mut self) -> PResult<RowExpr> {
        let start = self.span();
        if self.at(&TokenKind::LBrace) {
            let open = self.advance();
            let mut atoms = Vec::new();
            let mut aliases = Vec::new();
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Pipe) {
                if self.at_eof() {
                    return Err(self.unclosed(open, "`}` to close the effect row"));
                }
                match self.row_member("an effect atom, or the name of an `effect set`")? {
                    RowMember::Atom(a) => atoms.push(a),
                    RowMember::Set(q) => {
                        self.uses_effect_sets = true;
                        aliases.push(q);
                    }
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            let tail = if self.eat(&TokenKind::Pipe) {
                Some(self.expect_ident("an effect row variable after `|`")?)
            } else {
                None
            };
            let close =
                self.expect_close(&TokenKind::RBrace, open, "`}` to close the effect row")?;
            return Ok(RowExpr {
                atoms,
                aliases,
                tail,
                span: start.to(close),
            });
        }
        // A whole row that is a bare name is still a row *variable*: a set is only ever written
        // inside braces, so `/ e` keeps the meaning it has.
        let tail = self.expect_ident("an effect row: `{..}` or a row variable")?;
        Ok(RowExpr {
            atoms: Vec::new(),
            aliases: Vec::new(),
            span: tail.span,
            tail: Some(tail),
        })
    }

    /// One member of a row or of an `effect set`.
    fn row_member(&mut self, what: &str) -> PResult<RowMember> {
        let name = self.qname(what)?;
        if !self.at(&TokenKind::Dot) {
            return Ok(RowMember::Set(name));
        }
        self.advance();
        Ok(RowMember::Atom(self.atom_rest(name)?))
    }

    /// The atom after its effect name and the `.` have been consumed.
    fn atom_rest(&mut self, effect: QName) -> PResult<AtomExpr> {
        let mode = self.mode()?;
        let resource = if self.at(&TokenKind::LBracket) {
            let open = self.advance();
            let r = self.expect_ident("a resource name inside `[..]`")?;
            self.expect_close(&TokenKind::RBracket, open, "`]`")?;
            Some(r)
        } else {
            None
        };
        let span = effect.span.to(self.prev_span());
        Ok(AtomExpr {
            effect,
            mode,
            resource,
            span,
        })
    }

    fn expr(&mut self) -> PResult<Expr> {
        self.bin_expr(1)
    }

    fn scrutinee(&mut self) -> PResult<Expr> {
        let saved = std::mem::replace(&mut self.no_brace, true);
        let r = self.expr();
        self.no_brace = saved;
        r
    }

    /// The operator at the cursor, its binding power, and how many tokens it spans; only a shift
    /// is wider than one, because `>>` is not lexed and `Map<Int, List<Int>>` is why.
    fn peek_bin_op(&self) -> Option<(BinOp, u8, usize)> {
        // A lambda's parameter list ends in a `|`, and a parameter's default is an expression, so
        // `|x = 1| x` would otherwise read the closing pipe as bit-or and swallow the body.
        if self.no_pipe && matches!(self.kind(), TokenKind::Pipe) {
            return None;
        }
        match self.kind() {
            TokenKind::Gt if self.joined(2) => Some((BinOp::Ushr, SHIFT_BP, 3)),
            TokenKind::Gt if self.joined(1) => Some((BinOp::Shr, SHIFT_BP, 2)),
            TokenKind::Lt if self.joined(1) => Some((BinOp::Shl, SHIFT_BP, 2)),
            k => bin_op(k).map(|(op, bp)| (op, bp, 1)),
        }
    }

    fn bin_expr(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.unary_expr()?;
        while let Some((op, bp, width)) = self.peek_bin_op() {
            if bp < min_bp {
                break;
            }
            for _ in 0..width {
                self.advance();
            }
            let rhs = self.bin_expr(bp + 1)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn unary_expr(&mut self) -> PResult<Expr> {
        self.deeper()?;
        let r = self.unary_body();
        self.depth -= 1;
        r
    }

    fn unary_body(&mut self) -> PResult<Expr> {
        let op = match self.kind() {
            TokenKind::Minus => UnOp::Neg,
            TokenKind::Bang => UnOp::Not,
            TokenKind::Tilde => UnOp::BitNot,
            _ => return self.postfix_expr(),
        };
        let start = self.advance();
        let operand = self.unary_expr()?;
        let span = start.to(operand.span);
        Ok(Expr {
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            span,
        })
    }

    fn postfix_expr(&mut self) -> PResult<Expr> {
        let mut e = self.primary_expr()?;
        loop {
            match self.kind() {
                TokenKind::LParen => e = self.apply_to(e)?,
                TokenKind::Dot => {
                    // `db.get[users](k)` performs an effect; `r.f` reads a field.
                    let effect = match &e.kind {
                        ExprKind::Var(v)
                            if matches!(self.kind_at(1), TokenKind::Ident(_))
                                && matches!(
                                    self.kind_at(2),
                                    TokenKind::LBracket | TokenKind::LParen
                                ) =>
                        {
                            Some(v.clone())
                        }
                        _ => None,
                    };
                    self.advance();
                    let Some(effect) = effect else {
                        let field = self.expect_ident("a field name after `.`")?;
                        let span = e.span.to(field.span);
                        e = Expr {
                            kind: ExprKind::Field {
                                base: Box::new(e),
                                field,
                            },
                            span,
                        };
                        continue;
                    };
                    e = self.perform_on(e, effect)?;
                }
                // Tightest tier, alongside `f(x)` and `r.field`, so `f(x)?.g` is `(f(x)?).g` and
                // `-x?` is `-(x?)`
                TokenKind::Question => {
                    let close = self.advance();
                    let span = e.span.to(close);
                    self.uses_try = true;
                    e = Expr {
                        kind: ExprKind::Try {
                            operand: Box::new(e),
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn opt_resource(&mut self) -> PResult<Option<Ident>> {
        if !self.at(&TokenKind::LBracket) {
            return Ok(None);
        }
        let open = self.advance();
        let r = self.expect_ident("a resource name inside `[..]`")?;
        self.expect_close(&TokenKind::RBracket, open, "`]`")?;
        Ok(Some(r))
    }

    /// `f(..)`, out of line.
    #[inline(never)]
    fn apply_to(&mut self, func: Expr) -> PResult<Expr> {
        let (args, named, close) = self.call_args()?;
        let span = func.span.to(close);
        Ok(Expr {
            kind: ExprKind::App {
                func: Box::new(func),
                args,
                named,
            },
            span,
        })
    }

    /// `e.op[r](..)`, out of line for the reason [`Self::apply_to`] gives.
    #[inline(never)]
    fn perform_on(&mut self, base: Expr, effect: QName) -> PResult<Expr> {
        let op = self.expect_ident("an operation name")?;
        let resource = self.opt_resource()?;
        let (args, named, close) = self.call_args()?;
        // An operation has no defaults to fill and a handler clause must bind exactly what it
        // declares, so there is nothing for a name to select.
        if !named.is_empty() {
            self.no_named_arguments_on_a_perform(&effect, &op, &named);
        }
        let span = base.span.to(close);
        Ok(Expr {
            kind: ExprKind::Perform {
                effect,
                op,
                resource,
                args,
            },
            span,
        })
    }

    fn call_args(&mut self) -> PResult<(Vec<Expr>, Vec<NamedArg>, Span)> {
        let saved = std::mem::replace(&mut self.no_brace, false);
        let open = self.expect(&TokenKind::LParen, "`(` to start the argument list")?;
        let parsed = self.comma_list(&TokenKind::RParen, Self::call_arg);
        let r = parsed.and_then(|parsed| {
            let close =
                self.expect_close(&TokenKind::RParen, open, "`)` to close the arguments")?;
            let mut args = Vec::new();
            let mut named: Vec<NamedArg> = Vec::new();
            for arg in parsed {
                match arg {
                    Arg::Positional(e) => {
                        // Ordering is the parser's to enforce because it is a property of the text;
                        // which *names* are legal needs the callee's signature and is
                        // `defaults::expand`'s.
                        if let Some(first) = named.first() {
                            self.positional_after_named(e.span, first.span);
                        }
                        args.push(e);
                    }
                    Arg::Named(n) => named.push(n),
                }
            }
            Ok((args, named, close))
        });
        self.no_brace = saved;
        r
    }

    /// `name: value` is a named argument; anything else is positional.
    fn call_arg(&mut self) -> PResult<Arg> {
        if let TokenKind::Ident(name) = self.kind()
            && self.peek_is(1, &TokenKind::Colon)
        {
            let name = Ident::new(name.clone(), self.span());
            self.advance();
            self.advance();
            let value = self.expr()?;
            let span = name.span.to(value.span);
            return Ok(Arg::Named(NamedArg { name, value, span }));
        }
        Ok(Arg::Positional(self.expr()?))
    }

    fn primary_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::Int(v) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Lit(Lit::Int(v)),
                    span: start,
                })
            }
            TokenKind::Float(v) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Lit(Lit::Float(v)),
                    span: start,
                })
            }
            TokenKind::Decimal { mantissa, scale } => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Lit(Lit::Decimal { mantissa, scale }),
                    span: start,
                })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Lit(Lit::Str(s)),
                    span: start,
                })
            }
            TokenKind::Bytes(b) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Lit(Lit::Bytes(b)),
                    span: start,
                })
            }
            TokenKind::Kw(Kw::True) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Lit(Lit::Bool(true)),
                    span: start,
                })
            }
            TokenKind::Kw(Kw::False) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Lit(Lit::Bool(false)),
                    span: start,
                })
            }
            TokenKind::Ident(name) => {
                if name.as_str() == "with_cell" && matches!(self.kind_at(1), TokenKind::LBracket) {
                    return self.with_cell_expr();
                }
                if name.as_str() == "with_region" && matches!(self.kind_at(1), TokenKind::LBracket)
                {
                    return self.with_region_expr();
                }
                // Contextual, like `with_cell`: `simulate` stays an identifier everywhere else.
                if name.as_str() == "simulate"
                    && !self.no_brace
                    && matches!(self.kind_at(1), TokenKind::LBrace)
                {
                    return self.simulate_expr();
                }
                let q = self.qname("an expression")?;
                let span = q.span;
                Ok(Expr {
                    kind: ExprKind::Var(q),
                    span,
                })
            }
            TokenKind::LParen => {
                let saved = std::mem::replace(&mut self.no_brace, false);
                let open = self.advance();
                let r = if self.at(&TokenKind::RParen) {
                    let close = self.advance();
                    Ok(Expr {
                        kind: ExprKind::Lit(Lit::Unit),
                        span: open.to(close),
                    })
                } else {
                    self.expr().and_then(|mut inner| {
                        let close =
                            self.expect_close(&TokenKind::RParen, open, "`)` to close the group")?;
                        inner.span = open.to(close);
                        Ok(inner)
                    })
                };
                self.no_brace = saved;
                r
            }
            TokenKind::LBracket => {
                let saved = std::mem::replace(&mut self.no_brace, false);
                let open = self.advance();
                let r = self
                    .comma_list(&TokenKind::RBracket, Self::expr)
                    .and_then(|items| {
                        let close =
                            self.expect_close(&TokenKind::RBracket, open, "`]` to close the list")?;
                        Ok(Expr {
                            kind: ExprKind::List { items },
                            span: open.to(close),
                        })
                    });
                self.no_brace = saved;
                r
            }
            TokenKind::LBrace if !self.no_brace => {
                if self.at_record_literal() {
                    self.record_expr()
                } else {
                    self.block_expr()
                }
            }
            TokenKind::Pipe | TokenKind::PipePipe => self.lambda_expr(),
            TokenKind::Kw(Kw::If) => self.if_expr(),
            TokenKind::Kw(Kw::Match) => self.match_expr(),
            TokenKind::Kw(Kw::Handle) => self.handle_expr(),
            _ => Err(self.error_here("an expression")),
        }
    }

    /// `{x: e}` and `{x, y}` are records; `{x}` is a block whose value is `x`.
    fn at_record_literal(&self) -> bool {
        if matches!(self.kind_at(1), TokenKind::DotDot) {
            return true;
        }
        matches!(self.kind_at(1), TokenKind::Ident(_))
            && matches!(self.kind_at(2), TokenKind::Colon | TokenKind::Comma)
    }

    fn record_expr(&mut self) -> PResult<Expr> {
        let saved = std::mem::replace(&mut self.no_brace, false);
        let open = self.advance();
        let r = self.record_body(open);
        self.no_brace = saved;
        r
    }

    fn record_body(&mut self, open: Span) -> PResult<Expr> {
        let base = if self.at(&TokenKind::DotDot) {
            self.uses_record_update = true;
            let b = self.record_update_base()?;
            // `{..b}` is the whole expression when no comma follows; otherwise the comma separates
            // the base from the fields that replace.
            if !self.at(&TokenKind::RBrace) {
                self.expect(&TokenKind::Comma, "`,` after the record being updated")?;
            }
            Some(Box::new(b))
        } else {
            None
        };
        let fields = self.comma_list(&TokenKind::RBrace, Self::record_field)?;
        let close = self.expect_close(&TokenKind::RBrace, open, "`}` to close the record")?;
        let kind = match base {
            Some(base) => ExprKind::RecordUpdate { base, fields },
            None => ExprKind::Record { fields },
        };
        Ok(Expr {
            kind,
            span: open.to(close),
        })
    }

    /// The base of an update is a **path** — `s`, `state.limits` — and not an arbitrary expression.
    fn record_update_base(&mut self) -> PResult<Expr> {
        let dots = self.advance();
        if matches!(self.kind(), TokenKind::Dot) {
            // `...b`: `..` then `.`, which would otherwise report the useless "expected a name,
            // found `.`"
            let span = self.span();
            self.push(
                Diagnostic::error(
                    codes::UNEXPECTED_TOKEN,
                    "a record update is spelled `..`, not `...`",
                )
                .primary(dots.to(span), "one dot too many")
                .note("`..` is the same token the record *pattern* `{a, ..}` uses"),
            );
            return Err(Bail);
        }
        let name = self.expect_ident("the record being updated")?;
        let mut expr = Expr {
            span: name.span,
            kind: ExprKind::Var(name.into()),
        };
        while self.at(&TokenKind::Dot) {
            self.advance();
            let field = self.expect_ident("a field name")?;
            expr = Expr {
                span: expr.span.to(field.span),
                kind: ExprKind::Field {
                    base: Box::new(expr),
                    field,
                },
            };
        }
        Ok(expr)
    }

    fn record_field(&mut self) -> PResult<(Ident, Expr)> {
        if self.at(&TokenKind::DotDot) {
            let span = self.span();
            self.push(
                Diagnostic::error(
                    codes::UNEXPECTED_TOKEN,
                    "a record update has one base, written first",
                )
                .primary(span, "a second `..` here")
                .note("write `{..b, x: 1, y: 2}`; two bases have no defined order of fields"),
            );
            return Err(Bail);
        }
        let name = self.expect_ident("a field name")?;
        if self.eat(&TokenKind::Colon) {
            let value = self.expr()?;
            return Ok((name, value));
        }
        let span = name.span;
        Ok((
            name.clone(),
            Expr {
                kind: ExprKind::Var(name.into()),
                span,
            },
        ))
    }

    fn block_expr(&mut self) -> PResult<Expr> {
        let saved = std::mem::replace(&mut self.no_brace, false);
        let r = self.block_inner();
        self.no_brace = saved;
        r
    }

    fn block_inner(&mut self) -> PResult<Expr> {
        let open = self.expect(&TokenKind::LBrace, "`{` to start a block")?;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unclosed(open, "`}` to close the block"));
            }
            if self.at(&TokenKind::Kw(Kw::Let)) {
                stmts.push(self.let_stmt()?);
                continue;
            }
            let e = self.expr()?;
            if self.eat(&TokenKind::Semi) {
                stmts.push(Stmt::Expr(e));
            } else if self.at(&TokenKind::RBrace) {
                tail = Some(Box::new(e));
            } else if is_block_like(&e.kind) {
                stmts.push(Stmt::Expr(e));
            } else if self.at_eof() {
                return Err(self.unclosed(open, "`}` to close the block"));
            } else {
                return Err(self.error_here("`;` to end the statement, or `}` to end the block"));
            }
        }
        let close = self.expect_close(&TokenKind::RBrace, open, "`}` to close the block")?;
        Ok(Expr {
            kind: ExprKind::Block { stmts, tail },
            span: open.to(close),
        })
    }

    fn let_stmt(&mut self) -> PResult<Stmt> {
        let start = self.advance();
        let pat = self.pattern()?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.ty()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq, "`=` and an initializer")?;
        let value = self.expr()?;
        let semi = self.expect(&TokenKind::Semi, "`;` to end the `let`")?;
        Ok(Stmt::Let {
            pat,
            ty,
            value: Box::new(value),
            span: start.to(semi),
        })
    }

    fn lambda_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        let params = if self.eat(&TokenKind::PipePipe) {
            Vec::new()
        } else {
            self.advance();
            let saved = std::mem::replace(&mut self.no_pipe, true);
            let params = self.comma_list(&TokenKind::Pipe, Self::param);
            self.no_pipe = saved;
            let params = params?;
            self.expect(&TokenKind::Pipe, "`|` to close the lambda parameters")?;
            params
        };
        let saved = std::mem::replace(&mut self.no_pipe, false);
        let body = self.expr();
        self.no_pipe = saved;
        let body = body?;
        let span = start.to(body.span);
        Ok(Expr {
            kind: ExprKind::Lambda {
                params,
                body: Box::new(body),
            },
            span,
        })
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.advance();
        let cond = self.scrutinee()?;
        let then_branch = self.block_expr()?;
        let else_branch = if self.eat(&TokenKind::Kw(Kw::Else)) {
            if self.at(&TokenKind::Kw(Kw::If)) {
                self.if_expr()?
            } else {
                self.block_expr()?
            }
        } else {
            let end = then_branch.span.end;
            Expr {
                kind: ExprKind::Lit(Lit::Unit),
                span: Span::new(self.source, end, end),
            }
        };
        let span = start.to(else_branch.span);
        Ok(Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: Box::new(else_branch),
            },
            span,
        })
    }

    fn match_expr(&mut self) -> PResult<Expr> {
        let start = self.advance();
        let scrutinee = self.scrutinee()?;
        let saved = std::mem::replace(&mut self.no_brace, false);
        let r = self.match_arms();
        self.no_brace = saved;
        let (arms, close) = r?;
        Ok(Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: start.to(close),
        })
    }

    fn match_arms(&mut self) -> PResult<(Vec<MatchArm>, Span)> {
        let open = self.expect(&TokenKind::LBrace, "`{` to start the match arms")?;
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unclosed(open, "`}` to close the match arms"));
            }
            let pat = self.pattern()?;
            let guard = if self.eat(&TokenKind::Kw(Kw::If)) {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::Arrow, "`->` and the arm body")?;
            let body = self.expr()?;
            let span = pat.span.to(body.span);
            arms.push(MatchArm {
                pat,
                guard,
                body,
                span,
            });
            if !self.eat(&TokenKind::Comma) && !self.at(&TokenKind::RBrace) {
                let last = arms.last().expect("just pushed");
                if self.at_eof() {
                    return Err(self.unclosed(open, "`}` to close the match arms"));
                }
                if !is_block_like(&last.body.kind) {
                    return Err(self.error_here("`,` between match arms, or `}` to end the match"));
                }
            }
        }
        let close = self.expect_close(&TokenKind::RBrace, open, "`}` to close the match arms")?;
        Ok((arms, close))
    }

    fn handle_expr(&mut self) -> PResult<Expr> {
        let start = self.advance();
        let saved = std::mem::replace(&mut self.no_brace, false);
        let r = self.handle_rest(start);
        self.no_brace = saved;
        r
    }

    fn handle_rest(&mut self, start: Span) -> PResult<Expr> {
        let body = self.expr()?;
        self.expect(
            &TokenKind::Kw(Kw::With),
            "`with` and then the handler clauses",
        )?;
        let open = self.expect(&TokenKind::LBrace, "`{` to start the handler clauses")?;
        let mut clauses = Vec::new();
        let mut return_clause: Option<Box<ReturnClause>> = None;
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unclosed(open, "`}` to close the handler"));
            }
            if self.at_ident_text("return") && matches!(self.kind_at(1), TokenKind::Ident(_)) {
                let rc = self.return_clause()?;
                if let Some(prev) = &return_clause {
                    self.push(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            "a handler may have only one `return` clause",
                        )
                        .primary(rc.span, "duplicate `return` clause")
                        .secondary(prev.span, "first one is here"),
                    );
                } else {
                    return_clause = Some(Box::new(rc));
                }
            } else {
                clauses.push(self.handle_clause()?);
            }
            self.eat(&TokenKind::Comma);
        }
        let close = self.expect_close(&TokenKind::RBrace, open, "`}` to close the handler")?;
        Ok(Expr {
            kind: ExprKind::Handle {
                body: Box::new(body),
                clauses,
                return_clause,
            },
            span: start.to(close),
        })
    }

    fn handle_clause(&mut self) -> PResult<HandleClause> {
        let effect = self.qname("an effect name, or `return`")?;
        self.expect(&TokenKind::Dot, "`.` and the operation name")?;
        let op = self.expect_ident("an operation name")?;
        let resource = self.opt_resource()?;
        let open = self.expect(&TokenKind::LParen, "`(` to start the clause parameters")?;
        let params = self.comma_list(&TokenKind::RParen, Self::clause_param)?;
        self.expect_close(
            &TokenKind::RParen,
            open,
            "`)` to close the clause parameters",
        )?;
        // `resume` is a keyword only here, between a clause's `)` and its `->`.
        let resume = if self.at_ident_text("resume") {
            self.advance();
            Some(self.expect_ident("a name to bind the continuation to")?)
        } else {
            None
        };
        self.expect(&TokenKind::Arrow, "`->` and the clause body")?;
        let body = self.expr()?;
        let span = effect.span.to(body.span);
        Ok(HandleClause {
            effect,
            op,
            resource,
            params,
            resume,
            body,
            span,
        })
    }

    fn clause_param(&mut self) -> PResult<Ident> {
        self.expect_ident("a clause parameter name")
    }

    fn return_clause(&mut self) -> PResult<ReturnClause> {
        let start = self.advance();
        let binder = self.expect_ident("a binder after `return`")?;
        self.expect(&TokenKind::Arrow, "`->` and the `return` body")?;
        let body = self.expr()?;
        let span = start.to(body.span);
        Ok(ReturnClause { binder, body, span })
    }

    fn with_cell_expr(&mut self) -> PResult<Expr> {
        let start = self.advance();
        let bracket = self.expect(&TokenKind::LBracket, "`[` and a resource name")?;
        let resource = self.expect_ident("a resource name inside `[..]`")?;
        self.expect_close(&TokenKind::RBracket, bracket, "`]`")?;

        let saved = std::mem::replace(&mut self.no_brace, false);
        let r = self.with_cell_rest(start, resource);
        self.no_brace = saved;
        r
    }

    fn with_cell_rest(&mut self, start: Span, resource: Ident) -> PResult<Expr> {
        let paren = self.expect(&TokenKind::LParen, "`(` and the cell's initial value")?;
        let init = self.expr()?;
        self.expect_close(&TokenKind::RParen, paren, "`)` after the initial value")?;

        let brace = self.expect(&TokenKind::LBrace, "`{` to start the cell's region")?;
        let binder = self.expect_ident("a name to bind the cell to")?;
        self.expect(&TokenKind::Arrow, "`->` and the region body")?;
        let body = self.expr()?;
        let close =
            self.expect_close(&TokenKind::RBrace, brace, "`}` to close the cell's region")?;

        Ok(Expr {
            kind: ExprKind::WithCell {
                resource,
                init: Box::new(init),
                binder,
                body: Box::new(body),
            },
            span: start.to(close),
        })
    }

    fn with_region_expr(&mut self) -> PResult<Expr> {
        let start = self.advance();
        let bracket = self.expect(&TokenKind::LBracket, "`[` and a region name")?;
        let region = self.expect_ident("a region name inside `[..]`")?;
        self.expect_close(&TokenKind::RBracket, bracket, "`]`")?;

        let body = self.block_expr()?;
        let span = start.to(body.span);
        Ok(Expr {
            kind: ExprKind::WithRegion {
                region,
                body: Box::new(body),
            },
            span,
        })
    }

    fn simulate_expr(&mut self) -> PResult<Expr> {
        let start = self.advance();
        let body = self.block_expr()?;
        let span = start.to(body.span);
        Ok(Expr {
            kind: ExprKind::Simulate {
                body: Box::new(body),
            },
            span,
        })
    }

    fn pattern(&mut self) -> PResult<Pattern> {
        self.deeper()?;
        let r = self.pattern_body();
        self.depth -= 1;
        r
    }

    fn pattern_body(&mut self) -> PResult<Pattern> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::Underscore => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Wildcard,
                    span: start,
                })
            }
            TokenKind::Int(v) => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Lit(Lit::Int(v)),
                    span: start,
                })
            }
            TokenKind::Float(v) => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Lit(Lit::Float(v)),
                    span: start,
                })
            }
            TokenKind::Decimal { mantissa, scale } => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Lit(Lit::Decimal { mantissa, scale }),
                    span: start,
                })
            }
            // A negative literal is one pattern rather than an operator applied to one, because a
            // pattern is not an expression and there is nothing to apply.
            TokenKind::Minus
                if matches!(
                    self.kind_at(1),
                    TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::Decimal { .. }
                ) =>
            {
                self.advance();
                let lit = match self.kind().clone() {
                    TokenKind::Int(v) => Lit::Int(-v),
                    TokenKind::Float(v) => Lit::Float(-v),
                    TokenKind::Decimal { mantissa, scale } => Lit::Decimal {
                        mantissa: -mantissa,
                        scale,
                    },
                    _ => unreachable!(),
                };
                let end = self.advance();
                Ok(Pattern {
                    kind: PatternKind::Lit(lit),
                    span: start.to(end),
                })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Lit(Lit::Str(s)),
                    span: start,
                })
            }
            TokenKind::Bytes(b) => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Lit(Lit::Bytes(b)),
                    span: start,
                })
            }
            TokenKind::Kw(Kw::True) => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Lit(Lit::Bool(true)),
                    span: start,
                })
            }
            TokenKind::Kw(Kw::False) => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Lit(Lit::Bool(false)),
                    span: start,
                })
            }
            TokenKind::LParen => {
                let open = self.advance();
                if self.at(&TokenKind::RParen) {
                    let close = self.advance();
                    return Ok(Pattern {
                        kind: PatternKind::Lit(Lit::Unit),
                        span: open.to(close),
                    });
                }
                let mut inner = self.pattern()?;
                let close = self.expect_close(&TokenKind::RParen, open, "`)`")?;
                inner.span = open.to(close);
                Ok(inner)
            }
            TokenKind::LBracket => self.list_pattern(),
            TokenKind::LBrace => self.record_pattern(),
            TokenKind::Ident(_) => {
                let q = self.qname("a pattern")?;
                if q.is_bare() && !starts_upper(q.symbol()) {
                    return Ok(Pattern {
                        kind: PatternKind::Var(q.name),
                        span: start,
                    });
                }
                let mut args = Vec::new();
                let mut end = q.span;
                if self.at(&TokenKind::LParen) {
                    let open = self.advance();
                    args = self.comma_list(&TokenKind::RParen, Self::pattern)?;
                    end = self.expect_close(
                        &TokenKind::RParen,
                        open,
                        "`)` to close the constructor arguments",
                    )?;
                }
                Ok(Pattern {
                    kind: PatternKind::Ctor { name: q, args },
                    span: start.to(end),
                })
            }
            _ => Err(self.error_here("a pattern")),
        }
    }

    fn list_pattern(&mut self) -> PResult<Pattern> {
        let open = self.advance();
        let mut items = Vec::new();
        let mut rest = None;
        while !self.at(&TokenKind::RBracket) {
            if self.at_eof() {
                return Err(self.unclosed(open, "`]` to close the list pattern"));
            }
            if self.at(&TokenKind::DotDot) {
                let dots = self.advance();
                let bound = if matches!(self.kind(), TokenKind::Ident(_)) {
                    let name = self.expect_ident("a name after `..`")?;
                    let span = name.span;
                    Pattern {
                        kind: PatternKind::Var(name),
                        span,
                    }
                } else {
                    Pattern {
                        kind: PatternKind::Wildcard,
                        span: dots,
                    }
                };
                rest = Some(Box::new(bound));
                self.eat(&TokenKind::Comma);
                break;
            }
            items.push(self.pattern()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let close =
            self.expect_close(&TokenKind::RBracket, open, "`]` to close the list pattern")?;
        Ok(Pattern {
            kind: PatternKind::List { items, rest },
            span: open.to(close),
        })
    }

    fn record_pattern(&mut self) -> PResult<Pattern> {
        let open = self.advance();
        let mut fields = Vec::new();
        let mut has_rest = false;
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unclosed(open, "`}` to close the record pattern"));
            }
            if self.eat(&TokenKind::DotDot) {
                has_rest = true;
                self.eat(&TokenKind::Comma);
                break;
            }
            let name = self.expect_ident("a field name")?;
            let pat = if self.eat(&TokenKind::Colon) {
                self.pattern()?
            } else {
                let span = name.span;
                Pattern {
                    kind: PatternKind::Var(name.clone()),
                    span,
                }
            };
            fields.push((name, pat));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let close =
            self.expect_close(&TokenKind::RBrace, open, "`}` to close the record pattern")?;
        Ok(Pattern {
            kind: PatternKind::Record {
                fields,
                rest: has_rest,
            },
            span: open.to(close),
        })
    }

    fn comma_list<T>(
        &mut self,
        close: &TokenKind,
        item: fn(&mut Self) -> PResult<T>,
    ) -> PResult<Vec<T>> {
        let mut out = Vec::new();
        while !self.at(close) {
            if self.at_eof() {
                break;
            }
            out.push(item(self)?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(out)
    }
}

/// One implementation, in `ast`, because [`ast::is_default_expr`] asks the same question of a
/// default's callee that the grammar asks of a pattern.
use crate::ast::is_ctor_name as starts_upper;

/// Expressions that end in `}` may stand as a statement without a `;`.
fn is_block_like(kind: &ExprKind) -> bool {
    matches!(
        kind,
        ExprKind::Block { .. }
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Handle { .. }
            | ExprKind::WithCell { .. }
            | ExprKind::WithRegion { .. }
            | ExprKind::Simulate { .. }
    )
}

/// The one binding power no token carries on its own, since a shift is assembled from adjacent
/// `Gt`/`Lt` by [`Parser::peek_bin_op`].
const SHIFT_BP: u8 = 7;

/// Loosest to tightest, 1 to 10; the numbers renumbered when the bit operators took four levels
/// but no existing operator's relative order moved, so no program's parse tree did either.
fn bin_op(k: &TokenKind) -> Option<(BinOp, u8)> {
    Some(match k {
        TokenKind::PipePipe => (BinOp::Or, 1),
        TokenKind::AmpAmp => (BinOp::And, 2),
        TokenKind::EqEq => (BinOp::Eq, 3),
        TokenKind::BangEq => (BinOp::Ne, 3),
        TokenKind::Lt => (BinOp::Lt, 3),
        TokenKind::Le => (BinOp::Le, 3),
        TokenKind::Gt => (BinOp::Gt, 3),
        TokenKind::Ge => (BinOp::Ge, 3),
        TokenKind::Pipe => (BinOp::BitOr, 4),
        TokenKind::Caret => (BinOp::BitXor, 5),
        TokenKind::Amp => (BinOp::BitAnd, 6),
        TokenKind::PlusPlus => (BinOp::Concat, 8),
        TokenKind::Plus => (BinOp::Add, 9),
        TokenKind::Minus => (BinOp::Sub, 9),
        TokenKind::Star => (BinOp::Mul, 10),
        TokenKind::Slash => (BinOp::Div, 10),
        TokenKind::Percent => (BinOp::Rem, 10),
        _ => return None,
    })
}
