//! Errors are collected, not returned: a failed item is abandoned, the token
//! stream is resynchronised on the next item keyword, and parsing continues, so
//! one run reports as many independent syntax errors as it can find.

use crate::ast::*;
use crate::lexer::{Kw, Token, TokenKind, lex};
use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};

/// Signals that the enclosing construct was abandoned. The diagnostic explaining
/// why has already been recorded.
pub struct Bail;

type PResult<T> = Result<T, Bail>;

/// Recursive descent walks the Rust stack, so nesting has to be capped before it
/// overflows. Real code stays far below this; generated code need not.
const MAX_DEPTH: u32 = 128;

/// Parses a snippet as the anonymous module: it can neither import nor be
/// imported, which is all a test or an editor scratch buffer needs.
pub fn parse(source: SourceId, text: &str) -> Result<Module, Vec<Diagnostic>> {
    parse_module(source, ModuleName::anonymous(), text)
}

pub fn parse_module(
    source: SourceId,
    name: ModuleName,
    text: &str,
) -> Result<Module, Vec<Diagnostic>> {
    let (module, diags) = Parser::new(source, text).run(name);
    if diags.is_empty() { Ok(module) } else { Err(diags) }
}

/// Parses as much as possible and hands back both the partial tree and every
/// diagnostic. Editors and `--json` consumers want the tree even when it is
/// wrong; [`parse`] is the strict wrapper.
pub fn parse_recovering(
    source: SourceId,
    name: ModuleName,
    text: &str,
) -> (Module, Vec<Diagnostic>) {
    Parser::new(source, text).run(name)
}

/// Each input becomes its own module. Nothing is concatenated: a name in one
/// file is invisible in another until it is exported and imported.
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
    if diags.is_empty() { Ok(program) } else { Err(diags) }
}

pub fn parse_expr(source: SourceId, text: &str) -> Result<Expr, Vec<Diagnostic>> {
    let mut p = Parser::new(source, text);
    match p.expr() {
        Ok(e) if p.at(&TokenKind::Eof) && p.diags.is_empty() => Ok(e),
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
    /// Inside an `if`/`match` scrutinee a `{` starts the arm block, never a
    /// record or block expression. Reset by every opening delimiter.
    no_brace: bool,
    depth: u32,
}

impl Parser {
    fn new(source: SourceId, text: &str) -> Parser {
        let (tokens, diags) = lex(source, text);
        Parser { source, tokens, pos: 0, diags, no_brace: false, depth: 0 }
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
        if self.at(k) { Ok(self.advance()) } else { Err(self.error_here(what)) }
    }

    fn expect_close(&mut self, k: &TokenKind, open: Span, what: &str) -> PResult<Span> {
        if self.at(k) { Ok(self.advance()) } else { Err(self.unclosed(open, what)) }
    }

    fn unclosed(&mut self, open: Span, what: &str) -> Bail {
        let found = self.kind().describe();
        let span = self.span();
        self.push(
            Diagnostic::error(codes::UNEXPECTED_TOKEN, format!("expected {what}, found {found}"))
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

    /// `>=` immediately after a type parameter list is really `>` then `=`, as in
    /// `type Pair<a>= ..`. Split the token rather than reject the program.
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
        // A single mistake usually trips several expectations at the same
        // offset; only the first is informative.
        if let Some(last) = self.diags.last()
            && last.code == d.code
            && last.primary_span() == d.primary_span()
        {
            return;
        }
        self.diags.push(d);
    }

    fn error_here(&mut self, what: &str) -> Bail {
        let found = self.kind().describe();
        let span = self.span();
        self.push(
            Diagnostic::error(codes::UNEXPECTED_TOKEN, format!("expected {what}, found {found}"))
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
            Diagnostic::error(codes::UNEXPECTED_TOKEN, "input is nested too deeply to parse")
                .primary(span, format!("more than {MAX_DEPTH} levels of nesting"))
                .note("split this into smaller definitions"),
        );
        Err(Bail)
    }

    fn run(mut self, name: ModuleName) -> (Module, Vec<Diagnostic>) {
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
        (Module { name, source, imports, items }, self.diags)
    }

    /// Every `import` precedes every item, so the import table is complete
    /// before any body is parsed.
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

    /// Reported rather than silently accepted: a later pass would otherwise have
    /// to look ahead to know what a name in an earlier body could mean.
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

        Ok(ImportDecl { path, kind, span: start.to(self.prev_span()) })
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
        )
    }

    fn recover_to_item(&mut self) {
        let mut depth = 0i32;
        // Both callers consume the leading keyword before they can fail, so
        // already being at an item start means progress was made and the token
        // belongs to the next construct rather than the abandoned one.
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

    /// A qualified reference uses `::` rather than `.` because with `.` it would
    /// be token-identical to a perform and to a field access, and telling those
    /// apart needs scope information the parser does not have.
    fn qname(&mut self, what: &str) -> PResult<QName> {
        let first = self.expect_ident(what)?;
        if !self.eat(&TokenKind::ColonColon) {
            return Ok(QName::bare(first));
        }
        let name = self.expect_ident("a name after `::`")?;
        if self.at(&TokenKind::ColonColon) {
            let span = self.span();
            self.push(
                Diagnostic::error(codes::UNEXPECTED_TOKEN, "a qualified name has at most one `::`")
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
        let vis = if pub_span.is_some() { Visibility::Public } else { Visibility::Private };

        match self.kind() {
            TokenKind::Kw(Kw::Fn) => self.fn_def(vis).map(|d| Item::Fn(Box::new(d))),
            TokenKind::Kw(Kw::Type) => self.type_def(vis).map(|d| Item::Type(Box::new(d))),
            TokenKind::Kw(Kw::Effect) | TokenKind::Kw(Kw::Nondet) => {
                self.effect_def(vis).map(|d| Item::Effect(Box::new(d)))
            }
            TokenKind::Kw(Kw::Test) => {
                if let Some(span) = pub_span {
                    self.push(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            "a `test` cannot be `pub`",
                        )
                        .primary(span, "remove `pub`")
                        .note("a test has no name another module could reference"),
                    );
                }
                self.test_def().map(|d| Item::Test(Box::new(d)))
            }
            _ => Err(self.error_here("an item: `fn`, `type`, `effect`, `nondet effect`, or `test`")),
        }
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
        let params = self.comma_list(&TokenKind::RParen, Self::param)?;
        self.expect_close(&TokenKind::RParen, open, "`)` to close the parameter list")?;

        let ret = if self.eat(&TokenKind::Arrow) { Some(self.ty()?) } else { None };
        let effects = if self.eat(&TokenKind::Slash) { Some(self.row()?) } else { None };

        let body = if self.eat(&TokenKind::Eq) {
            self.expr()?
        } else if self.at(&TokenKind::LBrace) {
            self.block_expr()?
        } else {
            return Err(self.error_here("`=` or `{` to start the function body"));
        };

        let span = start.to(body.span);
        Ok(FnDef { vis, name, generics, params, ret, effects, body, span })
    }

    fn param(&mut self) -> PResult<Param> {
        let name = self.expect_ident("a parameter name")?;
        let ty = if self.eat(&TokenKind::Colon) { Some(self.ty()?) } else { None };
        let span = name.span.to(ty.as_ref().map_or(name.span, |t| t.span()));
        Ok(Param { name, ty, span })
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
        Ok(TypeDef { vis, name, params, body, span })
    }

    /// `type T = A` is an alias; a sum needs either a leading `|`, a payload, or
    /// a second variant, so that `type Id = Int` keeps meaning what it looks like.
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
        let close = self.expect_close(&TokenKind::RBrace, open, "`}` to close the operation list")?;
        Ok(EffectDef { vis, name, nondet, ops, span: start.to(close) })
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
        self.expect_close(&TokenKind::RParen, open, "`)` to close the operation parameters")?;
        self.expect(&TokenKind::Arrow, "`->` and a return type")?;
        let ret = self.ty()?;
        let span = start.to(ret.span());
        Ok(OpDef { name, mode, resource_param, params, ret, span })
    }

    /// An operation parameter may be written `name: Type` for documentation; only
    /// the type is part of the signature.
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
        Ok(TestDef { name, name_span, nondet, body, span })
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
                    return Ok(TypeExpr::Unit { span: open.to(close) });
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
                Ok(TypeExpr::Record { fields, span: open.to(close) })
            }
            TokenKind::Ident(_) => {
                let q = self.qname("a type")?;
                if self.at(&TokenKind::Lt) {
                    self.advance();
                    let args = self.comma_list(&TokenKind::Gt, Self::ty)?;
                    let close = self.expect_gt("`>` to close the type arguments")?;
                    return Ok(TypeExpr::Con { name: q, args, span: start.to(close) });
                }
                // A type parameter is bound by the enclosing `<..>`, never by a
                // module, so only a bare lowercase name can be one.
                if q.is_bare() && !starts_upper(q.symbol()) {
                    return Ok(TypeExpr::Var(q.name));
                }
                let span = q.span;
                Ok(TypeExpr::Con { name: q, args: Vec::new(), span })
            }
            _ => Err(self.error_here("a type")),
        }
    }

    fn fn_ty(&mut self, params: Vec<TypeExpr>, start: Span) -> PResult<TypeExpr> {
        let ret = self.ty()?;
        let effects = if self.eat(&TokenKind::Slash) { Some(self.row()?) } else { None };
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
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Pipe) {
                if self.at_eof() {
                    return Err(self.unclosed(open, "`}` to close the effect row"));
                }
                atoms.push(self.atom()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            let tail = if self.eat(&TokenKind::Pipe) {
                Some(self.expect_ident("an effect row variable after `|`")?)
            } else {
                None
            };
            let close = self.expect_close(&TokenKind::RBrace, open, "`}` to close the effect row")?;
            return Ok(RowExpr { atoms, tail, span: start.to(close) });
        }
        let tail = self.expect_ident("an effect row: `{..}` or a row variable")?;
        Ok(RowExpr { atoms: Vec::new(), span: tail.span, tail: Some(tail) })
    }

    fn atom(&mut self) -> PResult<AtomExpr> {
        let effect = self.qname("an effect name")?;
        self.expect(&TokenKind::Dot, "`.` and then `read` or `write`")?;
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
        Ok(AtomExpr { effect, mode, resource, span })
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

    fn bin_expr(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.unary_expr()?;
        while let Some((op, bp)) = bin_op(self.kind()) {
            if bp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.bin_expr(bp + 1)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) },
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
            _ => return self.postfix_expr(),
        };
        let start = self.advance();
        let operand = self.unary_expr()?;
        let span = start.to(operand.span);
        Ok(Expr { kind: ExprKind::Unary { op, operand: Box::new(operand) }, span })
    }

    fn postfix_expr(&mut self) -> PResult<Expr> {
        let mut e = self.primary_expr()?;
        loop {
            match self.kind() {
                TokenKind::LParen => {
                    let (args, close) = self.call_args()?;
                    let span = e.span.to(close);
                    e = Expr { kind: ExprKind::App { func: Box::new(e), args }, span };
                }
                TokenKind::Dot => {
                    // `db.get[users](k)` performs an effect; `r.f` reads a field.
                    // Only a name can be an effect, and an operation is always
                    // applied to a resource or an argument list.
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
                        e = Expr { kind: ExprKind::Field { base: Box::new(e), field }, span };
                        continue;
                    };
                    let op = self.expect_ident("an operation name")?;
                    let resource = self.opt_resource()?;
                    let (args, close) = self.call_args()?;
                    let span = e.span.to(close);
                    e = Expr { kind: ExprKind::Perform { effect, op, resource, args }, span };
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

    fn call_args(&mut self) -> PResult<(Vec<Expr>, Span)> {
        let saved = std::mem::replace(&mut self.no_brace, false);
        let open = self.expect(&TokenKind::LParen, "`(` to start the argument list")?;
        let args = self.comma_list(&TokenKind::RParen, Self::expr);
        let r = args.and_then(|args| {
            let close = self.expect_close(&TokenKind::RParen, open, "`)` to close the arguments")?;
            Ok((args, close))
        });
        self.no_brace = saved;
        r
    }

    fn primary_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::Int(v) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Lit(Lit::Int(v)), span: start })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Lit(Lit::Str(s)), span: start })
            }
            TokenKind::Kw(Kw::True) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Lit(Lit::Bool(true)), span: start })
            }
            TokenKind::Kw(Kw::False) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Lit(Lit::Bool(false)), span: start })
            }
            TokenKind::Ident(name) => {
                if name.as_str() == "with_cell" && matches!(self.kind_at(1), TokenKind::LBracket) {
                    return self.with_cell_expr();
                }
                let q = self.qname("an expression")?;
                let span = q.span;
                Ok(Expr { kind: ExprKind::Var(q), span })
            }
            TokenKind::LParen => {
                let saved = std::mem::replace(&mut self.no_brace, false);
                let open = self.advance();
                let r = if self.at(&TokenKind::RParen) {
                    let close = self.advance();
                    Ok(Expr { kind: ExprKind::Lit(Lit::Unit), span: open.to(close) })
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
                let r = self.comma_list(&TokenKind::RBracket, Self::expr).and_then(|items| {
                    let close =
                        self.expect_close(&TokenKind::RBracket, open, "`]` to close the list")?;
                    Ok(Expr { kind: ExprKind::List { items }, span: open.to(close) })
                });
                self.no_brace = saved;
                r
            }
            TokenKind::LBrace if !self.no_brace => {
                if self.at_record_literal() { self.record_expr() } else { self.block_expr() }
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
        matches!(self.kind_at(1), TokenKind::Ident(_))
            && matches!(self.kind_at(2), TokenKind::Colon | TokenKind::Comma)
    }

    fn record_expr(&mut self) -> PResult<Expr> {
        let saved = std::mem::replace(&mut self.no_brace, false);
        let open = self.advance();
        let r = self.comma_list(&TokenKind::RBrace, Self::record_field).and_then(|fields| {
            let close = self.expect_close(&TokenKind::RBrace, open, "`}` to close the record")?;
            Ok(Expr { kind: ExprKind::Record { fields }, span: open.to(close) })
        });
        self.no_brace = saved;
        r
    }

    fn record_field(&mut self) -> PResult<(Ident, Expr)> {
        let name = self.expect_ident("a field name")?;
        if self.eat(&TokenKind::Colon) {
            let value = self.expr()?;
            return Ok((name, value));
        }
        let span = name.span;
        Ok((name.clone(), Expr { kind: ExprKind::Var(name.into()), span }))
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
        Ok(Expr { kind: ExprKind::Block { stmts, tail }, span: open.to(close) })
    }

    fn let_stmt(&mut self) -> PResult<Stmt> {
        let start = self.advance();
        let pat = self.pattern()?;
        let ty = if self.eat(&TokenKind::Colon) { Some(self.ty()?) } else { None };
        self.expect(&TokenKind::Eq, "`=` and an initializer")?;
        let value = self.expr()?;
        let semi = self.expect(&TokenKind::Semi, "`;` to end the `let`")?;
        Ok(Stmt::Let { pat, ty, value: Box::new(value), span: start.to(semi) })
    }

    fn lambda_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        let params = if self.eat(&TokenKind::PipePipe) {
            Vec::new()
        } else {
            self.advance();
            let params = self.comma_list(&TokenKind::Pipe, Self::param)?;
            self.expect(&TokenKind::Pipe, "`|` to close the lambda parameters")?;
            params
        };
        let body = self.expr()?;
        let span = start.to(body.span);
        Ok(Expr { kind: ExprKind::Lambda { params, body: Box::new(body) }, span })
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.advance();
        let cond = self.scrutinee()?;
        let then_branch = self.block_expr()?;
        let else_branch = if self.eat(&TokenKind::Kw(Kw::Else)) {
            if self.at(&TokenKind::Kw(Kw::If)) { self.if_expr()? } else { self.block_expr()? }
        } else {
            let end = then_branch.span.end;
            Expr { kind: ExprKind::Lit(Lit::Unit), span: Span::new(self.source, end, end) }
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
            kind: ExprKind::Match { scrutinee: Box::new(scrutinee), arms },
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
            let guard = if self.eat(&TokenKind::Kw(Kw::If)) { Some(self.expr()?) } else { None };
            self.expect(&TokenKind::Arrow, "`->` and the arm body")?;
            let body = self.expr()?;
            let span = pat.span.to(body.span);
            arms.push(MatchArm { pat, guard, body, span });
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
        self.expect(&TokenKind::Kw(Kw::With), "`with` and then the handler clauses")?;
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
            kind: ExprKind::Handle { body: Box::new(body), clauses, return_clause },
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
        self.expect_close(&TokenKind::RParen, open, "`)` to close the clause parameters")?;
        self.expect(&TokenKind::Arrow, "`->` and the clause body")?;
        let body = self.expr()?;
        let span = effect.span.to(body.span);
        Ok(HandleClause { effect, op, resource, params, body, span })
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
        let close = self.expect_close(&TokenKind::RBrace, brace, "`}` to close the cell's region")?;

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
                Ok(Pattern { kind: PatternKind::Wildcard, span: start })
            }
            TokenKind::Int(v) => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Lit(Lit::Int(v)), span: start })
            }
            TokenKind::Minus if matches!(self.kind_at(1), TokenKind::Int(_)) => {
                self.advance();
                let TokenKind::Int(v) = *self.kind() else { unreachable!() };
                let end = self.advance();
                Ok(Pattern { kind: PatternKind::Lit(Lit::Int(-v)), span: start.to(end) })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Lit(Lit::Str(s)), span: start })
            }
            TokenKind::Kw(Kw::True) => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Lit(Lit::Bool(true)), span: start })
            }
            TokenKind::Kw(Kw::False) => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Lit(Lit::Bool(false)), span: start })
            }
            TokenKind::LParen => {
                let open = self.advance();
                if self.at(&TokenKind::RParen) {
                    let close = self.advance();
                    return Ok(Pattern { kind: PatternKind::Lit(Lit::Unit), span: open.to(close) });
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
                    return Ok(Pattern { kind: PatternKind::Var(q.name), span: start });
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
                Ok(Pattern { kind: PatternKind::Ctor { name: q, args }, span: start.to(end) })
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
                    Pattern { kind: PatternKind::Var(name), span }
                } else {
                    Pattern { kind: PatternKind::Wildcard, span: dots }
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
        let close = self.expect_close(&TokenKind::RBracket, open, "`]` to close the list pattern")?;
        Ok(Pattern { kind: PatternKind::List { items, rest }, span: open.to(close) })
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
                Pattern { kind: PatternKind::Var(name.clone()), span }
            };
            fields.push((name, pat));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let close =
            self.expect_close(&TokenKind::RBrace, open, "`}` to close the record pattern")?;
        Ok(Pattern {
            kind: PatternKind::Record { fields, rest: has_rest },
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

fn starts_upper(name: &Symbol) -> bool {
    name.as_str().chars().next().is_some_and(char::is_uppercase)
}

/// Expressions that end in `}` may stand as a statement without a `;`.
fn is_block_like(kind: &ExprKind) -> bool {
    matches!(
        kind,
        ExprKind::Block { .. }
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Handle { .. }
            | ExprKind::WithCell { .. }
    )
}

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
        TokenKind::PlusPlus => (BinOp::Concat, 4),
        TokenKind::Plus => (BinOp::Add, 5),
        TokenKind::Minus => (BinOp::Sub, 5),
        TokenKind::Star => (BinOp::Mul, 6),
        TokenKind::Slash => (BinOp::Div, 6),
        TokenKind::Percent => (BinOp::Rem, 6),
        _ => return None,
    })
}
