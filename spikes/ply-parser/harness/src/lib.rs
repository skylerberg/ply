//! The reference side of the parser spike's differential: `ply_syntax`'s tree,
//! written out in the same flat dump grammar `spikes/ply-parser/*.ply` emits.
//!
//! The grammar is specified once, in `spine.ply`'s "The dump encoder" comment,
//! and this file is the other half of it:
//!
//! ```text
//!   S:E:TAG;        one node, span S..E; TAG fixes how many children follow
//!   #K;             a list of exactly K children follows
//!   ?0; / ?1;       an absent / present Option; ?1 is followed by one child
//!   @HEX;           an inline scalar payload, hex-encoded
//!   %WORD;          an enum arm
//!   !CODE:S:E:L:N;  a diagnostic: code, primary span, label count, note count
//!   =S:E:F;         one of that diagnostic's labels; F is 1 for primary
//! ```
//!
//! Three properties make it structural rather than a bag of nodes, and each is
//! a corruption `../arm-harness.sh` performs and watches go red: every list
//! emits its length, so a dropped element shifts every record after it; every
//! `Option` emits presence, so a dropped `Option` cannot be absorbed; and every
//! node leads with **its own span**, which is the hazard the lexer spike names
//! first — a parser with every tag right and every offset wrong would agree
//! with a span-blind comparator perfectly.
//!
//! Every character is printable ASCII and neither `"` nor `\` occurs, because
//! `ply run --json` renders a `String` through `ply_eval::value::escape` and a
//! dump holding either would make the harness compare its own unescaper against
//! the parser. `the_dump_is_printable_ascii_with_no_quote_and_no_backslash`
//! pins that of this side.
//!
//! **Not one `match` here has a `_` arm.** That is the whole reason a tree
//! differential is worth more than a diagnostics-only one: a variant added to
//! `ast.rs` has to be given a dump here or this file stops compiling, where a
//! wildcard would silently emit nothing and the comparison would stay green
//! over a field nobody reached.
//!
//! > **And a `..` is a `_` for fields.** The sentence above is about *variants*
//! > and it was read as though it covered *fields*, which it does not: a
//! > struct-variant pattern that ends in `..` absorbs a new field exactly the
//! > way a `_` absorbs a new variant, in silence, with no compile error. That
//! > is what happened to `ExprKind::App`'s keyword-argument list (see
//! > `dumper_boundaries` below and `../GAPS.md` §11R.N), and `tests/fields.rs`
//! > did not catch it because a doc comment about effect sets happened to
//! > contain the word.
//! >
//! > **Re-counted 2026-08-30, after that hole was closed.** Four `..` patterns
//! > remain and **not one of them is in the dump**:
//! >
//! > * `Lit::Decimal { .. }` in `Dumper::lit`, whose two absorbed fields are
//! >   the two entries of `tests/fields.rs`'s `EXPECTED_ABSENT` — a decimal is
//! >   dumped as the source over its own span, deliberately, and if it grew a
//! >   third field that test would report it.
//! > * three in `Dumper::rows_of_ty`, which is the row **walk** and emits
//! >   nothing at all; its own comment says it answers a different question and
//! >   must not drift into the dump.
//! >
//! > The two that used to sit on the sugar nodes are gone with their
//! > `unreachable!()` arms, and the `App` one is gone with the hole. The rule
//! > this note exists to state is unchanged: **write out every field, and if
//! > you write `..`, say here why the compiler will never tell you it grew.**
//!
//! # What this comparison covers, and what it does not
//!
//! `../GAPS-harness.md` §H2 is the enforced list and `../GAPS.md` §11R.D is the
//! argument; [`dumper_boundaries`] is the one-screen version, kept next to the
//! code it describes so that changing the code and not the claim is awkward.

use ply_span::{Diagnostic, SourceId, Span};
use ply_syntax::ast::*;
use ply_syntax::parse_unexpanded;

/// **What the differential compares, and what it structurally cannot see.**
///
/// Empty on purpose: this is documentation that has to live beside the dumper,
/// because the failure it is about is a claim drifting away from the code that
/// was supposed to back it. `../GAPS-harness.md` §H2 is the enforced list;
/// `../GAPS.md` §11R.D is the argument for the boundary and §11R.X what taking
/// it cost; this is the summary.
///
/// # 1. Where the tree is read from, and why that is the whole question
///
/// [`reference_dump`] enters at [`ply_syntax::parse_unexpanded`]. That is
/// `Parser::new(source, text).run_unexpanded(name)`: the grammar and the
/// recovery loop, and **not** the three rewrites `Parser::run` performs after
/// them, each gated on a `uses_*` flag:
///
/// | pass | lines | rewrites |
/// | --- | ---: | --- |
/// | `effect_set::expand` | 538 | splices a set's atoms into every row that names it |
/// | `record_update::expand` | 530 | `{..b, f: e}` into a plain `Record` |
/// | `try_op::expand` | 1,019 | `e?` into the `match` it stands for |
///
/// A **fourth** pass, `defaults::expand` (912 lines), fills unwritten arguments
/// from the callee's signature and clears `App`'s keyword-argument list. It is
/// **not** a parser pass and cannot become one: a defaulted argument's
/// expression lives in the callee's *module*, so it needs the whole program,
/// and it therefore runs inside [`ply_syntax::resolve`] (`resolve.rs:453`) — a
/// phase this file never calls.
///
/// So the tree reaching this dumper is **pre** all four. Every boundary below
/// follows from that one sentence.
///
/// > **Corrected 2026-08-30.** This section said the tree was *"**post** the
/// > first three and **pre** the fourth"*, which was true of
/// > `parse_recovering`, the entry point this file used until that day. The
/// > port implements the grammar and none of the rewrites, so a post-rewrite
/// > comparison measured four things at once and reported 28 of 763 inputs
/// > disagreeing — 70.2% of the corpus by bytes — over sugar the port never
/// > claimed to expand. `../GAPS.md` §11R.D is the decision and its cost.
///
/// **A warning to whoever edits this comment.** It used to be worse than this:
/// `tests/fields.rs` read the whole of this file as a bag of words, so writing
/// the bare name of an `ast.rs` field anywhere here — a doc comment included —
/// told that test the field was covered, and drafting this very block once
/// flipped its verdict from "one field is not dumped" to "every field is
/// dumped". That test now strips `//` and `///` before matching, so prose here
/// is inert and the names below no longer have to be spelled around. **The
/// limit it does not close is still open**: naming a field is not emitting one,
/// so a field read into a binding and never pushed to the output is green
/// there. `../arm-harness.sh` is what catches that, and nothing else does.
///
/// # 2. What is compared
///
/// Every node in preorder with **its own span**; every list's length; every
/// `Option`'s presence; every enum arm; every scalar payload. Then every
/// diagnostic's code, primary span, label count, note count, and each label's
/// own span and primary flag — 828 diagnostics over the 766-input corpus, with
/// **no tolerance of any kind**. The one this harness used to grant, for
/// `effect_set::expand`'s appended diagnostics, is deleted along with the pass
/// that made it necessary.
///
/// # 3. What is not, in the order it costs
///
/// 1. **The three rewrites above — 2,087 lines of Rust that nothing in this
///    spike tests.** This is the largest item and it is permanent under the
///    decision. It is *measured*, not asserted:
///    `tests/agreement.rs`'s
///    `the_rewrites_this_comparison_gives_up_raise_exactly_these_diagnostics`
///    reports what the three add to the corpus — **7 diagnostics** (E0114 ×4,
///    E0115 ×2, E0105 ×1, on 7 mined fixtures) and **3,974 nodes**
///    (`db.ply` 2,137, `desk.ply` 1,028, `http.ply` 279, `json.ply` 119,
///    `router.ply` 11, `config.ply` 11) — and pins the diagnostic figure so it
///    cannot grow quietly. All seven were already excused by the tolerance the
///    move deleted, so **no diagnostic this differential ever compared was
///    given up**. `record_update` and `try_op` raise none anywhere in the
///    corpus, so no error path of theirs was ever verified here.
/// 2. **`defaults::expand`**, which was never in the comparison and cannot be,
///    for the reason in §1. Note the direction: this is why the keyword
///    arguments and fallback expressions below are *live* in this tree rather
///    than already placed.
/// 3. **Diagnostic message text and severity.** ~134 sites carry a
///    `what: Bytes` that nothing reads. Every parser diagnostic is
///    `Severity::Error`, so a warning added to the parser would be invisible.
/// 4. **`FnDef::derived`** — the parser can only write `None`, and this file
///    **asserts** that rather than skipping it.
/// 5. **`Lit::Decimal`'s two numeric fields, and `Lit::Float`'s value.** Both
///    sides dump the raw source over the literal's own span, because Ply can
///    build neither an `f64` nor an `i128` from digits. This *removes* a
///    normaliser where the lexer spike's float hole added one.
///
/// > **Two items left this list on 2026-08-30 and are recorded because what
/// > they cost is the point.** `ExprKind::App`'s keyword-argument list was
/// > absorbed by a `..` and emitted **nowhere**: `g(1, b: 2)`, `g(1, c: 2)` and
/// > `g(1, b: h(2))` produced byte-identical dumps, so a port that read three
/// > tokens and threw them away would have passed. `Param`'s
/// > fallback-expression field was the same feature's other half. Both are
/// > dumped now — the first as a `narg` node with its own span inside a
/// > length-carrying list, the second as an `Option` under `prm` — and
/// > `../arm-harness.sh` #17 and #20 are the two mutations that watch them.
/// > Neither is reached by the mined corpus, which contains zero of either;
/// > both are reached by `../fixtures/13-named-arguments-and-defaults.ply`,
/// > which is therefore load-bearing.
///
/// # 4. The two tests that guard this list, and the exact limit of each
///
/// * **No `match` in this file has a `_` arm**, so a variant added to `ast.rs`
///   stops the file compiling. It did: `RecordUpdate` and `Try` broke the build
///   when they landed, which is the bit-rot `../README.md` §6 predicted.
///   **The limit: `..` is a `_` for fields, and no compiler error attends it.**
///   That limit cost this comparison a whole field for two days.
/// * **`tests/fields.rs`** reads `ast.rs` and requires every field of every
///   parsed type to be *named* in this file, with comments stripped from both
///   sides first. **The limit, measured:** naming is not emitting — renaming a
///   binding to `init: i0` keeps it green.
pub mod dumper_boundaries {}

/// The whole answer for one file: the tree, then every diagnostic in the order
/// the parser raised them.
///
/// `text` is kept because two of the dump's leaves are **source slices** rather
/// than values: `Lit::Float` and `Lit::Decimal` are dumped as the raw bytes
/// under the literal node's own span. Ply cannot build an `f64` from digits at
/// all, so a dump carrying the value would be comparing `f64::from_str` against
/// nothing. `patterns.ply`'s header fixes the rule for both sides, and it
/// closes the hole `spikes/ply-lexer/README.md` names by removing the
/// normalisation from the comparison rather than adding a third normaliser.
pub fn reference_dump(text: &str) -> String {
    let (module, diags) = parse_unexpanded(SourceId(0), ModuleName::anonymous(), text);
    // The one observable difference between the two entry points that is a
    // *field* rather than a node: `effect_set::expand`'s `write_back` fills this
    // in, so an empty one is evidence the pass did not run. Asserted rather than
    // assumed, for `Dumper::fn_def`'s reason — if the pass ever moves back
    // inside the grammar, the comparison should stop rather than quietly agree
    // about a list that both sides happen to leave empty.
    for item in &module.items {
        if let Item::EffectSet(d) = item {
            assert!(
                d.expansion.is_empty(),
                "`parse_unexpanded` handed back an effect set whose `expansion` is filled \
                 in, so `effect_set::expand` has run and this is no longer the tree the \
                 grammar built"
            );
        }
    }
    dump_of(text, &module, &diags)
}

/// The dump of one already-parsed module. Split out of [`reference_dump`] so
/// that [`nodes_the_rewrites_add`] can point it at a tree from the *other*
/// entry point without a second copy of the encoder.
fn dump_of(text: &str, module: &Module, diags: &[Diagnostic]) -> String {
    let mut d = Dumper {
        text,
        out: String::new(),
    };
    d.list(&module.imports, Dumper::import);
    d.list(&module.items, Dumper::item);
    d.diags(diags);
    d.out
}

/// **The tree half of the same cost: how many nodes the three rewrites add.**
///
/// [`diagnostics_the_rewrites_add`] says what leaves the comparison on the
/// diagnostic side. This says it on the node side, which is the side `../GAPS.md`
/// §11R.D could only state in *lines of Rust*: `effect_set` splices a set's
/// atoms into every row that names it, `record_update` writes one field copy per
/// field it did not replace, and `try_op` writes a whole `match`. All three only
/// ever add, so this is non-negative, and it is exactly the tree this
/// differential does not look at.
///
/// The expanded tree is dumped with the **same** encoder, which is sound: it
/// holds no `Try` and no `RecordUpdate` — that is what expansion means — so
/// every arm it reaches is one that was there before those two variants existed.
pub fn nodes_the_rewrites_add(text: &str) -> usize {
    let (before, bd) = parse_unexpanded(SourceId(0), ModuleName::anonymous(), text);
    let (after, ad) = ply_syntax::parse_recovering(SourceId(0), ModuleName::anonymous(), text);
    let b = node_count(&dump_of(text, &before, &bd));
    let a = node_count(&dump_of(text, &after, &ad));
    assert!(
        a >= b,
        "the rewrites removed {} node(s), which none of them can do",
        b - a
    );
    a - b
}

// > **WITHDRAWN 2026-08-30 — the projection is gone, not merely unused.**
// > `reference_dump_unexpanded` stood here, and `../GAPS-harness.md` §H4 was
// > its cost. It read:
// >
// > > *"The same dump with `effect_set::expand`'s effect on the **tree**
// > > projected back out: every row's atoms truncated to the ones that were
// > > written, and every set's `expansion` emitted empty. This exists because
// > > `items.ply` does not port the expander … and `examples/desk.ply` is 21%
// > > of the corpus by bytes and the one file in the tree that uses sets.
// > > Without this the honest options are to drop that file or to compare it
// > > against a pass the port does not have; this is the third, and it is a
// > > projection of the reference's *own output*, not a re-implementation: …
// > > `expand` appends to `row.atoms`, in order, one set's `expansion` per
// > > entry of `row.aliases`. Those expansions are sitting in the tree, filled
// > > in by `write_back`, so how many atoms were appended is read off rather
// > > than recomputed … It is **not** applied to diagnostics: `expand` raises
// > > `E0105`, `E0114` and `E0115` of its own, `items.ply` raises `E0114` for
// > > its own reasons, and telling those apart by code would be guessing."*
// >
// > Every sentence of that was true of a comparison entered at
// > `parse_recovering`. This file enters at `parse_unexpanded`, where
// > `effect_set::expand` **has not run**, so there is nothing to project: the
// > rows already hold only the atoms that were written and every set's
// > `expansion` is already empty. `Dumper::effect_set` now *asserts* that
// > rather than emitting a zero for it, which is strictly stronger — a
// > projection is a claim about a pass, an assertion is a check on one.
// >
// > What went with it: the whole diagnostic tolerance the last paragraph
// > describes. `tests/agreement.rs`'s `only_the_expanders_diagnostics` was four
// > conjuncts wide and excused 7 mined inputs; `expand` raising nothing means
// > there is nothing to excuse, and the comparison is now exact on every
// > diagnostic of every input. `../GAPS-harness.md` §H4 records the arithmetic.

/// **What the pre-expansion comparison gives up, as data rather than as prose.**
///
/// Every diagnostic code `parse_recovering` raises for this input that
/// [`parse_unexpanded`] does not, in order. Those are exactly the diagnostics
/// the three rewrites raise, because the two entry points differ by nothing
/// else: `Parser::run` is `Parser::run_unexpanded` plus three gated calls, and
/// each can only append.
///
/// This is the **only** place in the harness that reaches
/// `ply_syntax::parse_recovering`, and it reaches it to measure a cost, never
/// to compare against it. It answers codes and not a dump, deliberately: a dump
/// would invite somebody to diff it, and the whole argument of `../GAPS.md`
/// §11R.D is that the post-rewrite tree is a different subject.
///
/// `tests/agreement.rs`'s `the_rewrites_this_comparison_gives_up_raise_exactly_
/// these_diagnostics` is the caller, and it pins the total over the corpus.
pub fn diagnostics_the_rewrites_add(text: &str) -> Vec<String> {
    let (_, before) = parse_unexpanded(SourceId(0), ModuleName::anonymous(), text);
    let (_, after) = ply_syntax::parse_recovering(SourceId(0), ModuleName::anonymous(), text);
    assert!(
        after.len() >= before.len(),
        "the rewrites removed a diagnostic, which no pass can do: {} before, {} after",
        before.len(),
        after.len()
    );
    after[before.len()..]
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

/// Whether this file would have had `effect_set::expand` run over it, had the
/// comparison entered at `parse_recovering`.
///
/// Kept after the projection went, and its job changed: it used to *select* the
/// projected dump, and it now only names the files whose comparison used to be
/// weaker than the rest. `tests/agreement.rs`'s
/// `the_one_file_that_used_to_need_a_projection_is_now_compared_whole` is the
/// only caller, and it asserts that the set is still exactly `desk.ply` — so if
/// a second file starts using effect sets, the test that says "and it is
/// compared whole like everything else" is re-read rather than silently widened.
///
/// This mirrors `parser.rs`'s own `uses_effect_sets` from the outside: the flag
/// is set by writing an `effect set` item or by naming one in a row, and after
/// the parse those are exactly a non-empty `EffectSetDef` list and a non-empty
/// `RowExpr::aliases`.
pub fn uses_effect_sets(text: &str) -> bool {
    let (module, _) = parse_unexpanded(SourceId(0), ModuleName::anonymous(), text);
    let mut found = module.items.iter().any(|i| matches!(i, Item::EffectSet(_)));
    walk_rows(&module, &mut |r: &RowExpr| {
        if !r.aliases.is_empty() {
            found = true;
        }
    });
    found
}

/// Every `RowExpr` the module holds, in the order the dump reaches them.
fn walk_rows(module: &Module, f: &mut impl FnMut(&RowExpr)) {
    let mut d = Dumper {
        text: "",
        out: String::new(),
    };
    d.rows_of_module(module, f);
}

struct Dumper<'a> {
    text: &'a str,
    out: String,
}

impl<'a> Dumper<'a> {
    // --- the encoder's five terminals ---------------------------------------

    fn rec(&mut self, span: Span, tag: &str) {
        self.out
            .push_str(&format!("{}:{}:{};", span.start, span.end, tag));
    }

    fn nlist(&mut self, k: usize) {
        self.out.push_str(&format!("#{k};"));
    }

    fn opt_flag(&mut self, present: bool) {
        self.out.push_str(if present { "?1;" } else { "?0;" });
    }

    fn word(&mut self, w: &str) {
        self.out.push('%');
        self.out.push_str(w);
        self.out.push(';');
    }

    fn payload(&mut self, bytes: &[u8]) {
        self.out.push('@');
        for b in bytes {
            self.out.push_str(&format!("{b:02x}"));
        }
        self.out.push(';');
    }

    fn list<T>(&mut self, xs: &[T], f: impl Fn(&mut Self, &T)) {
        self.nlist(xs.len());
        for x in xs {
            f(self, x);
        }
    }

    fn opt<T>(&mut self, o: Option<&T>, f: impl Fn(&mut Self, &T)) {
        match o {
            None => self.opt_flag(false),
            Some(x) => {
                self.opt_flag(true);
                f(self, x);
            }
        }
    }

    /// The bytes of the source under a node's own span, which is what the Ply
    /// side's `src_over` answers.
    fn src_over(&self, span: Span) -> &'a str {
        &self.text[span.start as usize..span.end as usize]
    }

    // --- leaves -------------------------------------------------------------

    fn ident(&mut self, i: &Ident) {
        self.rec(i.span, "ident");
        self.payload(i.name.as_str().as_bytes());
    }

    fn qname(&mut self, q: &QName) {
        self.rec(q.span, "qname");
        self.opt(q.module.as_ref(), Self::ident);
        self.ident(&q.name);
    }

    fn boolean(&mut self, b: bool) {
        self.word(if b { "true" } else { "false" });
    }

    fn vis(&mut self, v: Visibility) {
        match v {
            Visibility::Public => self.word("pub"),
            Visibility::Private => self.word("priv"),
        }
    }

    fn mode(&mut self, m: Mode) {
        match m {
            Mode::Read => self.word("read"),
            Mode::Write => self.word("write"),
        }
    }

    fn deriver(&mut self, d: Deriver) {
        match d {
            Deriver::Json => self.word("json"),
            Deriver::Eq => self.word("eq"),
            Deriver::Ord => self.word("ord"),
        }
    }

    fn spec_kind(&mut self, k: SpecKind) {
        match k {
            SpecKind::Requires => self.word("requires"),
            SpecKind::Ensures => self.word("ensures"),
        }
    }

    /// `span` is the *literal node's* span, not the token's: a negative literal
    /// in a pattern is one `PatternKind::Lit` covering the `-` as well, and
    /// `patterns.ply` dumps the source over that wider span. Passing the token's
    /// span here would agree on every positive number and differ on every
    /// negative one, which is exactly the class of bug the spans exist to catch.
    fn lit(&mut self, l: &Lit, span: Span) {
        match l {
            Lit::Int(v) => {
                self.word("int");
                self.payload(v.to_string().as_bytes());
            }
            Lit::Bool(b) => {
                self.word("bool");
                self.payload(if *b { b"1" } else { b"0" });
            }
            Lit::Str(s) => {
                self.word("str");
                self.payload(s.as_bytes());
            }
            Lit::Bytes(b) => {
                self.word("bytes");
                self.payload(b);
            }
            Lit::Float(_) => {
                self.word("float");
                let raw = self.src_over(span).as_bytes().to_vec();
                self.payload(&raw);
            }
            Lit::Decimal { .. } => {
                self.word("dec");
                let raw = self.src_over(span).as_bytes().to_vec();
                self.payload(&raw);
            }
            Lit::Unit => self.word("unit"),
        }
    }

    // --- types --------------------------------------------------------------

    fn ty(&mut self, t: &TypeExpr) {
        match t {
            TypeExpr::Var(i) => {
                self.rec(i.span, "tvar");
                self.ident(i);
            }
            TypeExpr::Con { name, args, span } => {
                self.rec(*span, "tcon");
                self.qname(name);
                self.list(args, Self::ty);
            }
            TypeExpr::Fn {
                params,
                ret,
                effects,
                span,
            } => {
                self.rec(*span, "tfn");
                self.list(params, Self::ty);
                self.ty(ret);
                self.opt(effects.as_ref(), Self::row);
            }
            TypeExpr::Record { fields, span } => {
                self.rec(*span, "trec");
                self.list(fields, |d, (n, t)| {
                    d.ident(n);
                    d.ty(t);
                });
            }
            TypeExpr::Unit { span } => self.rec(*span, "tuni"),
        }
    }

    /// Every atom the row **wrote**, which is every atom it holds: no set has
    /// been spliced into it, because `effect_set::expand` did not run.
    fn row(&mut self, r: &RowExpr) {
        self.rec(r.span, "row");
        self.list(&r.atoms, Self::atom);
        self.list(&r.aliases, Self::qname);
        self.opt(r.tail.as_ref(), Self::ident);
    }

    fn atom(&mut self, a: &AtomExpr) {
        self.rec(a.span, "atm");
        self.qname(&a.effect);
        self.mode(a.mode);
        self.opt(a.resource.as_ref(), Self::ident);
    }

    /// `Generics` carries no span in `ast.rs`, so this leads with a word rather
    /// than a record. Both lists still carry their length.
    fn generics(&mut self, g: &Generics) {
        self.word("gen");
        self.list(&g.types, Self::ident);
        self.list(&g.effects, Self::ident);
    }

    /// The fallback expression is dumped like any other `Option`, and that is a
    /// change: it arrived with ADR 0029 and nothing emitted it until
    /// `../GAPS.md` §11R.D moved this comparison to the pre-rewrite tree.
    /// `tests/fields.rs` was failing on it, which is why `../run.sh` stopped
    /// before reaching the differential at all (§11R.S).
    fn param(&mut self, p: &Param) {
        self.rec(p.span, "prm");
        self.ident(&p.name);
        self.opt(p.ty.as_ref(), Self::ty);
        self.opt(p.default.as_ref(), Self::expr);
    }

    // --- patterns -----------------------------------------------------------

    fn pattern(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Wildcard => self.rec(p.span, "pwld"),
            PatternKind::Var(i) => {
                self.rec(p.span, "pvar");
                self.ident(i);
            }
            PatternKind::Lit(l) => {
                self.rec(p.span, "plit");
                self.lit(l, p.span);
            }
            PatternKind::Ctor { name, args } => {
                self.rec(p.span, "pctr");
                self.qname(name);
                self.list(args, Self::pattern);
            }
            PatternKind::Record { fields, rest } => {
                self.rec(p.span, "prec");
                self.opt_flag(*rest);
                self.list(fields, |d, (n, p)| {
                    d.ident(n);
                    d.pattern(p);
                });
            }
            PatternKind::List { items, rest } => {
                self.rec(p.span, "plst");
                self.list(items, Self::pattern);
                self.opt(rest.as_deref(), Self::pattern);
            }
        }
    }

    // --- expressions --------------------------------------------------------

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lit(l) => {
                self.rec(e.span, "elit");
                self.lit(l, e.span);
            }
            ExprKind::Var(q) => {
                self.rec(e.span, "evar");
                self.qname(q);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.rec(e.span, "ebin");
                self.word(bin_op_name(*op));
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Unary { op, operand } => {
                self.rec(e.span, "eun");
                self.word(un_op_name(*op));
                self.expr(operand);
            }
            ExprKind::Lambda { params, body } => {
                self.rec(e.span, "elam");
                self.list(params, Self::param);
                self.expr(body);
            }
            // Every `name: value` argument, with its own span, its name and
            // its value — and the list's length, so a call that dropped one
            // could not be absorbed.
            //
            // Not dumping this was the defect `../GAPS.md` §11R.N measured:
            // `g(1, b: 2)`, `g(1, c: 3)` and `g(1, b: h(2))` produced
            // byte-identical dumps, so a port that read the three tokens and
            // threw them away would have passed. The comment this replaces
            // argued the field was empty here — "`defaults::expand` places
            // every named argument in `resolve`, which runs before anything
            // here sees a tree" — and had the phase order backwards:
            // `defaults::expand` runs in `resolve`, which is *after* this and
            // which this file never calls.
            //
            // Note what did NOT fix it: `tests/fields.rs` reported `named` as
            // covered throughout, because the word appears in a doc comment
            // about effect sets. That test now strips comments (see its
            // header); this arm is what actually emits the field.
            ExprKind::App { func, args, named } => {
                self.rec(e.span, "eapp");
                self.expr(func);
                self.list(args, Self::expr);
                self.list(named, Self::named_arg);
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.rec(e.span, "eif");
                self.expr(cond);
                self.expr(then_branch);
                self.expr(else_branch);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.rec(e.span, "emat");
                self.expr(scrutinee);
                self.list(arms, Self::arm);
            }
            ExprKind::Block { stmts, tail } => {
                self.rec(e.span, "eblk");
                self.list(stmts, Self::stmt);
                self.opt(tail.as_deref(), Self::expr);
            }
            ExprKind::Record { fields } => {
                self.rec(e.span, "erec");
                self.list(fields, |d, (n, v)| {
                    d.ident(n);
                    d.expr(v);
                });
            }
            ExprKind::Field { base, field } => {
                self.rec(e.span, "efld");
                self.expr(base);
                self.ident(field);
            }
            ExprKind::List { items } => {
                self.rec(e.span, "elst");
                self.list(items, Self::expr);
            }
            ExprKind::Perform {
                effect,
                op,
                resource,
                args,
            } => {
                self.rec(e.span, "eprf");
                self.qname(effect);
                self.ident(op);
                self.opt(resource.as_ref(), Self::ident);
                self.list(args, Self::expr);
            }
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                self.rec(e.span, "ehnd");
                self.expr(body);
                self.list(clauses, Self::handle_clause);
                self.opt(return_clause.as_deref(), Self::return_clause);
            }
            ExprKind::WithCell {
                resource,
                init,
                binder,
                body,
            } => {
                self.rec(e.span, "ecel");
                self.ident(resource);
                self.expr(init);
                self.ident(binder);
                self.expr(body);
            }
            ExprKind::WithRegion { region, body } => {
                self.rec(e.span, "ergn");
                self.ident(region);
                self.expr(body);
            }
            ExprKind::Simulate { body } => {
                self.rec(e.span, "esim");
                self.expr(body);
            }

            // > **Withdrawn 2026-08-30.** These two arms were
            // > `unreachable!()`, under: *"Two nodes that cannot reach a
            // > dumper: both are sugar the parser expands before
            // > `parse_recovering` returns (ADR 0023, ADR 0028) … The arms
            // > exist because this `match` has no `_`, which is what stopped
            // > this file compiling when the two variants were added -- the
            // > bit-rot `README.md` §6 predicted, working as designed."*
            // >
            // > Every clause of that is still true **of `parse_recovering`**,
            // > and this file no longer enters there: it enters at
            // > `parse_unexpanded`, where both nodes are exactly what the
            // > grammar built and neither rewrite has run. So the two arms
            // > that were unreachable are now the two that carry 70.2% of the
            // > corpus by bytes. `../GAPS.md` §11R.D is the decision.
            ExprKind::RecordUpdate { base, fields } => {
                self.rec(e.span, "erup");
                self.expr(base);
                self.list(fields, |d, (n, v)| {
                    d.ident(n);
                    d.expr(v);
                });
            }
            ExprKind::Try { operand } => {
                self.rec(e.span, "etry");
                self.expr(operand);
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let {
                pat,
                ty,
                value,
                span,
            } => {
                self.rec(*span, "slet");
                self.pattern(pat);
                self.opt(ty.as_ref(), Self::ty);
                self.expr(value);
            }
            Stmt::Expr(e) => {
                self.rec(e.span, "sexp");
                self.expr(e);
            }
        }
    }

    /// A named argument carries **its own span** as well as its name and its
    /// value: `E0123` and `E0124` both point at exactly that span, and nothing
    /// else in the dump would pin it.
    fn named_arg(&mut self, n: &NamedArg) {
        self.rec(n.span, "narg");
        self.ident(&n.name);
        self.expr(&n.value);
    }

    fn arm(&mut self, m: &MatchArm) {
        self.rec(m.span, "arm");
        self.pattern(&m.pat);
        self.opt(m.guard.as_ref(), Self::expr);
        self.expr(&m.body);
    }

    fn handle_clause(&mut self, h: &HandleClause) {
        self.rec(h.span, "hcl");
        self.qname(&h.effect);
        self.ident(&h.op);
        self.opt(h.resource.as_ref(), Self::ident);
        self.list(&h.params, Self::ident);
        self.opt(h.resume.as_ref(), Self::ident);
        self.expr(&h.body);
    }

    fn return_clause(&mut self, r: &ReturnClause) {
        self.rec(r.span, "rcl");
        self.ident(&r.binder);
        self.expr(&r.body);
    }

    // --- items --------------------------------------------------------------

    fn import(&mut self, d: &ImportDecl) {
        self.rec(d.span, "imp");
        self.list(&d.path, Self::ident);
        match &d.kind {
            ImportKind::Module => self.word("mod"),
            ImportKind::Alias(a) => {
                self.word("alias");
                self.ident(a);
            }
            ImportKind::Names(ns) => {
                self.word("names");
                self.list(ns, Self::ident);
            }
        }
    }

    fn constraint(&mut self, c: &Constraint) {
        self.rec(c.span, "cst");
        self.deriver(c.deriver);
        self.rec(c.deriver_span, "dsp");
        self.ident(&c.param);
    }

    fn spec(&mut self, s: &SpecClause) {
        self.rec(s.span, "spc");
        self.spec_kind(s.kind);
        self.expr(&s.expr);
    }

    fn fn_def(&mut self, d: &FnDef) {
        // `FnDef::derived` is written `None` at `parser.rs:723` and nothing in
        // the parser can produce anything else, so `items.ply` does not carry
        // it. **Asserted rather than skipped**: a field reached and not emitted
        // is precisely the survivor a dump-injectivity sweep exists to find,
        // and if a later change makes the parser fill it in, this is what says
        // so instead of the comparison staying quietly green.
        assert!(
            d.derived.is_none(),
            "the parser filled in `FnDef::derived`, which the Ply port does not carry; \
             the dump grammar has to gain a field before this comparison means anything"
        );
        self.rec(d.span, "fn");
        self.vis(d.vis);
        self.ident(&d.name);
        self.generics(&d.generics);
        self.list(&d.params, Self::param);
        self.opt(d.ret.as_ref(), Self::ty);
        self.opt(d.effects.as_ref(), Self::row);
        self.list(&d.constraints, Self::constraint);
        self.list(&d.spec, Self::spec);
        self.expr(&d.body);
    }

    fn binder(&mut self, b: &Binder) {
        self.rec(b.span, "bnd");
        self.ident(&b.name);
        self.ty(&b.ty);
    }

    fn law(&mut self, d: &LawDef) {
        self.rec(d.span, "law");
        self.rec(d.name_span, "lnm");
        self.payload(d.name.as_bytes());
        self.boolean(d.host);
        self.list(&d.binders, Self::binder);
        self.opt(d.guard.as_ref(), Self::expr);
        self.expr(&d.body);
    }

    fn variant(&mut self, v: &VariantDef) {
        self.rec(v.span, "var");
        self.ident(&v.name);
        self.list(&v.fields, Self::ty);
    }

    fn type_def(&mut self, d: &TypeDef) {
        self.rec(d.span, "ty");
        self.vis(d.vis);
        self.ident(&d.name);
        self.list(&d.params, Self::ident);
        match &d.body {
            TypeDefBody::Alias(t) => {
                self.word("alias");
                self.ty(t);
            }
            TypeDefBody::Sum(vs) => {
                self.word("sum");
                self.list(vs, Self::variant);
            }
        }
    }

    fn op_def(&mut self, o: &OpDef) {
        self.rec(o.span, "op");
        self.ident(&o.name);
        self.mode(o.mode);
        self.boolean(o.resource_param);
        self.list(&o.params, Self::ty);
        self.ty(&o.ret);
    }

    fn effect_def(&mut self, d: &EffectDef) {
        self.rec(d.span, "eff");
        self.vis(d.vis);
        self.ident(&d.name);
        self.boolean(d.nondet);
        self.list(&d.ops, Self::op_def);
    }

    fn test_def(&mut self, d: &TestDef) {
        self.rec(d.span, "tst");
        self.rec(d.name_span, "tnm");
        self.payload(d.name.as_bytes());
        self.boolean(d.nondet);
        self.expr(&d.body);
    }

    fn derive_def(&mut self, d: &DeriveDef) {
        self.rec(d.span, "der");
        self.deriver(d.deriver);
        self.rec(d.deriver_span, "dsp");
        self.ident(&d.target);
    }

    /// The `expansion` list is `effect_set::expand`'s own output, written back
    /// into the tree by `write_back`, so entering at `parse_unexpanded` makes it
    /// always empty. The **check** that it is lives at [`reference_dump`] rather
    /// than here, because it is a claim about which entry point was used and not
    /// about how a node is encoded — [`nodes_the_rewrites_add`] deliberately
    /// dumps an expanded tree through this same encoder, and must not trip it.
    fn effect_set(&mut self, d: &EffectSetDef) {
        self.rec(d.span, "set");
        self.ident(&d.name);
        self.list(&d.atoms, Self::atom);
        self.list(&d.includes, Self::qname);
        self.list(&d.expansion, Self::atom);
    }

    fn item(&mut self, i: &Item) {
        match i {
            Item::Fn(d) => self.fn_def(d),
            Item::Type(d) => self.type_def(d),
            Item::Effect(d) => self.effect_def(d),
            Item::Test(d) => self.test_def(d),
            Item::Law(d) => self.law(d),
            Item::Derive(d) => self.derive_def(d),
            Item::EffectSet(d) => self.effect_set(d),
        }
    }

    // --- diagnostics --------------------------------------------------------

    fn diags(&mut self, ds: &[Diagnostic]) {
        self.nlist(ds.len());
        for d in ds {
            let s = d.primary_span().unwrap_or(Span::DUMMY);
            self.out.push_str(&format!(
                "!{}:{}:{}:{}:{};",
                d.code,
                s.start,
                s.end,
                d.labels.len(),
                d.notes.len()
            ));
            for l in &d.labels {
                self.out.push_str(&format!(
                    "={}:{}:{};",
                    l.span.start,
                    l.span.end,
                    if l.primary { 1 } else { 0 }
                ));
            }
        }
    }

    // --- the row walk, for `uses_effect_sets` --------------------------------
    //
    // Separate from the dump because it answers a different question and must
    // not be allowed to drift into one: it reaches every `RowExpr` a module
    // holds, and nothing else.

    fn rows_of_module(&mut self, m: &Module, f: &mut impl FnMut(&RowExpr)) {
        for i in &m.items {
            match i {
                Item::Fn(d) => {
                    if let Some(r) = &d.effects {
                        f(r);
                    }
                    for p in &d.params {
                        if let Some(t) = &p.ty {
                            self.rows_of_ty(t, f);
                        }
                    }
                    if let Some(t) = &d.ret {
                        self.rows_of_ty(t, f);
                    }
                }
                Item::Type(d) => match &d.body {
                    TypeDefBody::Alias(t) => self.rows_of_ty(t, f),
                    TypeDefBody::Sum(vs) => {
                        for v in vs {
                            for t in &v.fields {
                                self.rows_of_ty(t, f);
                            }
                        }
                    }
                },
                Item::Effect(d) => {
                    for o in &d.ops {
                        for t in &o.params {
                            self.rows_of_ty(t, f);
                        }
                        self.rows_of_ty(&o.ret, f);
                    }
                }
                Item::Test(_) | Item::Law(_) | Item::Derive(_) | Item::EffectSet(_) => {}
            }
        }
    }

    fn rows_of_ty(&mut self, t: &TypeExpr, f: &mut impl FnMut(&RowExpr)) {
        match t {
            TypeExpr::Var(_) | TypeExpr::Unit { .. } => {}
            TypeExpr::Con { args, .. } => {
                for a in args {
                    self.rows_of_ty(a, f);
                }
            }
            TypeExpr::Fn {
                params,
                ret,
                effects,
                ..
            } => {
                for p in params {
                    self.rows_of_ty(p, f);
                }
                self.rows_of_ty(ret, f);
                if let Some(r) = effects {
                    f(r);
                }
            }
            TypeExpr::Record { fields, .. } => {
                for (_, ft) in fields {
                    self.rows_of_ty(ft, f);
                }
            }
        }
    }
}

fn bin_op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Rem => "rem",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Le => "le",
        BinOp::Gt => "gt",
        BinOp::Ge => "ge",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Concat => "concat",
    }
}

fn un_op_name(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "neg",
        UnOp::Not => "not",
    }
}

/// The dump as a list of records, for a diff that names the first disagreement
/// instead of printing two multi-megabyte strings.
pub fn records(dump: &str) -> Vec<&str> {
    dump.split_terminator(';').collect()
}

/// How many `S:E:TAG;` records the dump holds — the node count, which is what a
/// corpus figure has to state alongside the byte count. `#K;`, `?F;`, `%W;`,
/// `@HEX;`, `!..;` and `=..;` are structure, not nodes.
pub fn node_count(dump: &str) -> usize {
    records(dump)
        .iter()
        .filter(|r| !r.is_empty() && !r.starts_with(['#', '?', '%', '@', '!', '=']))
        .count()
}

/// Every distinct node tag the dump reached, sorted — the tag-coverage
/// statistic, so "agrees on the corpus" can be read next to "and the corpus
/// reaches these 30 of the 44 tags this grammar can emit".
pub fn tags(dump: &str) -> Vec<String> {
    let mut v: Vec<String> = records(dump)
        .iter()
        .filter(|r| !r.is_empty() && !r.starts_with(['#', '?', '@', '!', '=']))
        .map(|r| {
            if let Some(w) = r.strip_prefix('%') {
                format!("%{w}")
            } else {
                r.rsplit(':').next().unwrap_or("").to_string()
            }
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The fixtures in a bundle file, in order.
///
/// Everything before the first `%%` line is a header. After that, a line
/// holding exactly `%%` separates fixtures, and the newline immediately before
/// a separator — or before end of file — belongs to the separator rather than
/// to the fixture. `../mine-fixtures.py`'s doc comment says why that rule earns
/// its keep: `fn f() = "oops` unterminated at end of input and the same text
/// followed by a newline are two different arms of `lexer.rs`, and a bundle
/// that appended a newline would quietly test only one of them.
pub fn bundle(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.split_inclusive('\n') {
        if line.trim_end_matches('\n') == "%%" {
            if let Some(c) = cur.take() {
                out.push(strip_one_newline(c));
            }
            cur = Some(String::new());
        } else if let Some(c) = cur.as_mut() {
            c.push_str(line);
        }
    }
    if let Some(c) = cur {
        out.push(strip_one_newline(c));
    }
    out
}

fn strip_one_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

/// One diagnostic, read back out of a dump: code, primary span, every label,
/// and the note count.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DumpedDiag {
    pub code: String,
    pub span: (u32, u32),
    pub notes: usize,
    pub labels: Vec<(u32, u32, bool)>,
}

/// A dump split into its tree and its diagnostics.
///
/// The dump ends with `#K;` and then exactly K diagnostics and nothing else, so
/// this takes the **last** `#K;` whose tail parses as exactly that. Scanning
/// from the end rather than walking the tree forwards means this stays right
/// without a second copy of the grammar's arity table — the thing that would
/// have to be kept in step with `spine.ply` by hand and would not be.
///
/// `None` when no suffix parses, which is a malformed dump and must be loud.
pub fn split_diags(dump: &str) -> Option<(&str, Vec<DumpedDiag>)> {
    let mut starts: Vec<usize> = Vec::new();
    for (i, _) in dump.match_indices('#') {
        starts.push(i);
    }
    for &at in starts.iter().rev() {
        let rest = &dump[at..];
        let Some(semi) = rest.find(';') else { continue };
        let Ok(k) = rest[1..semi].parse::<usize>() else {
            continue;
        };
        if let Some(ds) = read_diags(&rest[semi + 1..], k) {
            return Some((&dump[..at], ds));
        }
    }
    None
}

fn read_diags(mut tail: &str, k: usize) -> Option<Vec<DumpedDiag>> {
    let mut out = Vec::with_capacity(k);
    for _ in 0..k {
        let body = tail.strip_prefix('!')?;
        let end = body.find(';')?;
        let f: Vec<&str> = body[..end].split(':').collect();
        if f.len() != 5 {
            return None;
        }
        let nlabels: usize = f[3].parse().ok()?;
        let mut d = DumpedDiag {
            code: f[0].to_string(),
            span: (f[1].parse().ok()?, f[2].parse().ok()?),
            notes: f[4].parse().ok()?,
            labels: Vec::with_capacity(nlabels),
        };
        tail = &body[end + 1..];
        for _ in 0..nlabels {
            let lb = tail.strip_prefix('=')?;
            let e = lb.find(';')?;
            let g: Vec<&str> = lb[..e].split(':').collect();
            if g.len() != 3 {
                return None;
            }
            d.labels
                .push((g[0].parse().ok()?, g[1].parse().ok()?, g[2] == "1"));
            tail = &lb[e + 1..];
        }
        out.push(d);
    }
    if tail.is_empty() { Some(out) } else { None }
}

/// A `b"..."` literal holding exactly these bytes, for embedding a source file
/// in a generated Ply program. Ply is the only way in: there is no file-reading
/// host handler, so a source file reaches a Ply program as a literal or not at
/// all.
pub fn byte_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 16);
    out.push_str("b\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dump_is_printable_ascii_with_no_quote_and_no_backslash() {
        let dump = reference_dump(
            "import a.b (c)\n\
             fn f(x: Int) -> Str / {db.read[users]} = \"a\\nb\" ++ b\"\\xff\"\n\
             test \"t\" { 1.5e-3 + 2.50m }\n\
             type T = | A(Int) | B\n",
        );
        for c in dump.chars() {
            assert!(
                c.is_ascii_graphic() && c != '"' && c != '\\',
                "the dump holds {c:?}, which `ply run --json` would escape:\n{dump}"
            );
        }
    }

    #[test]
    fn a_float_is_dumped_as_the_source_under_its_own_span() {
        // Not as `f64` bits: Ply cannot build one. The negative case is the
        // one that catches a dumper reaching for the token instead of the node.
        let dump = reference_dump("fn f() = match 0 { -1.5e-3 -> 1 }\n");
        assert!(dump.contains("%float;@2d312e35652d33;"), "{dump}");
    }

    #[test]
    fn a_decimal_is_dumped_as_the_source_under_its_own_span() {
        let dump = reference_dump("fn f() = 2.50m\n");
        assert!(dump.contains("%dec;@322e35306d;"), "{dump}");
    }

    #[test]
    fn every_list_carries_its_length_and_every_option_its_presence() {
        // `fn f()` with no return type, no row, no constraints, no spec.
        let dump = reference_dump("fn f() = 1\n");
        assert_eq!(
            dump,
            "#0;#1;0:10:fn;%priv;3:4:ident;@66;%gen;#0;#0;#0;?0;?0;#0;#0;9:10:elit;%int;@31;#0;"
        );
    }

    #[test]
    fn a_diagnostic_carries_every_label_and_the_note_count() {
        let dump = reference_dump("derive frobnicate for Order\n");
        let tail = &dump[dump.find("!E0207").expect("the unknown-deriver code")..];
        assert!(tail.starts_with("!E0207:7:17:1:1;=7:17:1;"), "{tail}");
    }

    #[test]
    fn node_count_counts_records_and_not_structure() {
        // one `fn`, one `ident`, one `elit`.
        assert_eq!(node_count(&reference_dump("fn f() = 1\n")), 3);
    }

    #[test]
    fn a_file_that_names_no_set_is_not_expanded() {
        assert!(!uses_effect_sets("fn f() -> Int / {db.read} = 1\n"));
        assert!(uses_effect_sets("effect set S = {a.read}\n"));
        assert!(uses_effect_sets("fn f() -> Int / {Web} = 1\n"));
    }

    #[test]
    fn a_dump_splits_into_its_tree_and_its_diagnostics() {
        let dump = reference_dump("derive frobnicate for Order\nfn f() = 1\n");
        let (tree, ds) = split_diags(&dump).expect("a well-formed dump");
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].code, "E0207");
        assert_eq!(ds[0].labels, vec![(7, 17, true)]);
        assert!(tree.ends_with("@31;"), "{tree}");
        // A tree holding its own `#K;` runs must not be mistaken for the block:
        // the split is the last suffix that parses, so an empty diagnostic list
        // after a tree full of lists still lands in the right place.
        let clean = reference_dump("fn f() = [1, 2, 3]\n");
        let (_, none) = split_diags(&clean).expect("a well-formed dump");
        assert!(none.is_empty());
    }

    #[test]
    fn a_bundle_gives_back_exactly_the_fixtures_that_were_written() {
        // The three cases the separator rule exists for: no trailing newline,
        // one trailing newline, and empty.
        let text = "header\nlines\n%%\nfn f() = 1\n%%\nfn g() = 2\n\n%%\n\n";
        assert_eq!(bundle(text), vec!["fn f() = 1", "fn g() = 2\n", ""]);
    }

    #[test]
    fn a_byte_literal_round_trips_every_byte_through_the_real_lexer() {
        let all: Vec<u8> = (0u8..=255).collect();
        let source = byte_literal(&all);
        let (tokens, diags) = ply_syntax::lexer::lex(SourceId(0), &source);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tokens[0].kind, ply_syntax::lexer::TokenKind::Bytes(all));
    }
}
