#!/usr/bin/env bash
# Every green in `spine.ply`'s tests, seen to go red.
#
# `CONTRIBUTING.md`'s first-named defect is a green result over unexplored
# space, and `spine.ply` passed all 18 of its first tests on the first run,
# which is exactly when that defect is invisible. This script corrupts one
# thing at a time and asserts the suite fails.
#
# It found six tests that were watching nothing, and they are named in
# `GAPS-spine.md` §S6 with what was wrong with each.
#
# `equiv` is the other half: a mutation that is *supposed* to leave the suite
# green, because the mutant is semantically equal to the original. An
# equivalent mutant is not a hole in a test, and calling one a hole is how a
# suite grows tests that assert an implementation detail. Each is asserted to
# stay green, so if it ever stops being equivalent the script says so.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }

# The spine is tested in a project of its own — `lexer.ply` and `spine.ply` and
# nothing else — rather than in `spikes/ply-parser/`. Four agents write into
# that directory at once and `ply test <dir>` typechecks every module in it, so
# a syntax error in an area still being written would otherwise read as the
# spine going red and every mutation below would "arm" for the wrong reason.
work="$(mktemp -d)"
cp "$here/lexer.ply" "$here/spine.ply" "$work/"
spine="$work/spine.ply"
backup="$work/spine.ply.orig"
cp "$spine" "$backup"
restore() { cp "$backup" "$spine"; }
trap 'rm -rf "$work"' EXIT

fails=0
mutate() {
  restore
  perl -0pi -e "s/\Q$1\E/$2/" "$spine" || return 1
  grep -qF -- "$2" "$spine"
}

arm() {
  local name="$1"
  if ! mutate "$2" "$3"; then
    echo "MUTATION DID NOT LAND: $name"; fails=$((fails + 1)); return
  fi
  if "$ply" test "$work" --no-cache >/dev/null 2>&1; then
    echo "NOT ARMED: $name -- the suite stayed green"
    fails=$((fails + 1))
  else
    echo "armed:    $name"
  fi
}

equiv() {
  local name="$1"
  if ! mutate "$2" "$3"; then
    echo "MUTATION DID NOT LAND: $name"; fails=$((fails + 1)); return
  fi
  if "$ply" test "$work" --no-cache >/dev/null 2>&1; then
    echo "equiv:    $name"
  else
    echo "NO LONGER EQUIVALENT: $name -- the suite went red"
    fails=$((fails + 1))
  fi
}

echo "==> the suite is green before any mutation"
"$ply" test "$work" --no-cache >/dev/null 2>&1 || { echo "the suite is RED to begin with"; exit 1; }

# 1. The expect_gt rewrite. Without it a `type Pair<a>= ..` loses its `=`.
arm "gt_split: the split token is never applied on read" \
  'if j == p.gt_split {' \
  'if false {'

arm "gt_split: the split = keeps the whole >= span" \
  '{ start: t.start + 1, end: t.end, tok: lexer::TPunct(b"eq") }' \
  '{ start: t.start, end: t.end, tok: lexer::TPunct(b"eq") }'

arm "expect_gt answers the whole >= as the >" \
  'node: {start: s.start, end: s.start + 1} }' \
  'node: {start: s.start, end: s.end} }'

# 2. Rule 1's guard, at each function that touches a token before its first
#    guarded callee. This failure mode has no analogue in the lexer spike:
#    GAPS.md §12 records that error accumulation cost the lexer nothing
#    because a lexer never fails.
arm "bail guard deleted from expect" \
  'pub fn expect(c: Ctx, p: P, k: lexer::Tok, what: Bytes) -> R<Span> =
  if p.bail {' \
  'pub fn expect(c: Ctx, p: P, k: lexer::Tok, what: Bytes) -> R<Span> =
  if false {'

arm "bail guard deleted from expect_close" \
  'pub fn expect_close(c: Ctx, p: P, k: lexer::Tok, open: Span, what: Bytes) -> R<Span> =
  if p.bail {' \
  'pub fn expect_close(c: Ctx, p: P, k: lexer::Tok, open: Span, what: Bytes) -> R<Span> =
  if false {'

arm "bail guard deleted from expect_ident" \
  'pub fn expect_ident(c: Ctx, p: P, what: Bytes) -> R<Ident> =
  if p.bail {' \
  'pub fn expect_ident(c: Ctx, p: P, what: Bytes) -> R<Ident> =
  if false {'

arm "bail guard deleted from expect_gt" \
  'pub fn expect_gt(c: Ctx, p: P, what: Bytes) -> R<Span> =
  if p.bail {' \
  'pub fn expect_gt(c: Ctx, p: P, what: Bytes) -> R<Span> =
  if false {'

arm "bail guard deleted from eat" \
  'pub fn eat(c: Ctx, p: P, k: lexer::Tok) -> Ate =
  if p.bail {' \
  'pub fn eat(c: Ctx, p: P, k: lexer::Tok) -> Ate =
  if false {'

arm "bail guard deleted from deeper" \
  'pub fn deeper(c: Ctx, p: P) -> P =
  if p.bail { p } else {' \
  'pub fn deeper(c: Ctx, p: P) -> P =
  if false { p } else {'

# The second equivalent mutant, for the same reason as `qname`'s: the
# `iterate` step's own first test is `s.p.bail`, so a bailed `comma_list` with
# no entry guard stops on round one with an empty list and an untouched state.
equiv "bail guard deleted from comma_list (redundant behind the step's own test)" \
  'pub fn comma_list<a>(c: Ctx, p: P, close: lexer::Tok, item: (Ctx, P) -> R<a>) -> R<List<a>> =
  if p.bail {' \
  'pub fn comma_list<a>(c: Ctx, p: P, close: lexer::Tok, item: (Ctx, P) -> R<a>) -> R<List<a>> =
  if false {'

# `qname`'s own guard is an EQUIVALENT MUTANT and this is the interesting
# result of the whole script. `qname` touches no token before calling
# `expect_ident`, which is guarded; `eat` is guarded; so with the entry guard
# deleted, a bailed `qname` still consumes nothing and reports nothing. The
# guard is therefore not defence, it is *the discipline that makes the
# invariant checkable without a call-graph argument*. It also means the
# guard-deletion instrument is weaker than a raw count of ~93 guards suggests:
# every guard on a function whose first act is a guarded call is unkillable.
equiv "bail guard deleted from qname (redundant behind expect_ident's)" \
  'pub fn qname(c: Ctx, p: P, what: Bytes) -> R<QName> =
  if p.bail {' \
  'pub fn qname(c: Ctx, p: P, what: Bytes) -> R<QName> =
  if false {'

# 3. Parser::push's dedup rule, which changes the diagnostic list exactly.
arm "dedup rule: every diagnostic is kept" \
  'if p.last_code == d.code && p.last_span.start == s.start && p.last_span.end == s.end {' \
  'if false {'

arm "dedup rule: keyed on the code alone, not the span" \
  'p.last_code == d.code && p.last_span.start == s.start && p.last_span.end == s.end' \
  'p.last_code == d.code'

arm "the lexer's diagnostics do not seed the dedup key" \
  'bail: false, last_code: d.code, last_span: d.span, diags: d.out' \
  'bail: false, last_code: b"", last_span: dummy_span(), diags: d.out'

# 4. The sequence driver, in the ways it can be subtly wrong.
arm "comma_list: a bailed element is pushed rather than abandoned" \
  'if r.p.bail {
          Stop({p: r.p, node: s.out})' \
  'if r.p.bail {
          Stop({p: r.p, node: push(s.out, r.node)})'

arm "comma_list: the last element before the closer is dropped" \
  'Stop({p: e.p, node: push(s.out, r.node)})' \
  'Stop({p: e.p, node: s.out})'

arm "comma_list: end of input is not a stop condition" \
  'if s.p.bail || at(c, s.p, close) || at_eof(c, s.p) {' \
  'if s.p.bail || at(c, s.p, close) {'

arm "comma_list: a missing comma does not end the list" \
  'let e = eat(c, r.p, t_comma());
          if e.ok {' \
  'let e = eat(c, r.p, t_comma());
          if true {'

# 5. Token access, where an off-by-one is invisible to a span-blind comparator.
arm "advance runs off the end of the buffer at EOF" \
  'if p.pos + 1 < c.ntok { at_pos(p, p.pos + 1) } else { p }' \
  'at_pos(p, p.pos + 1)'

arm "prev_span does not saturate at position zero" \
  'let j = if i < 0 { 0 } else if i > c.ntok - 1 { c.ntok - 1 } else { i };' \
  'let j = if i > c.ntok - 1 { c.ntok - 1 } else { i };'

arm "lookahead past the end does not clamp" \
  'else if i > c.ntok - 1 { c.ntok - 1 } else { i };' \
  'else { i };'

# 6. Contextual keywords. `law`, `derive`, `as`, `with_cell`, `with_region`,
#    `simulate` and `resume` all open a construct as identifiers and stay
#    usable as ordinary names, so an `at_ident_text` that also matched a
#    keyword would change what programs mean.
arm "at_ident_text matches a keyword as well as an identifier" \
  'pub fn at_ident_text(c: Ctx, p: P, text: Bytes) -> Bool =
  kind(c, p) == lexer::TIdent(text)' \
  'pub fn at_ident_text(c: Ctx, p: P, text: Bytes) -> Bool =
  kind(c, p) == lexer::TIdent(text) || kind(c, p) == lexer::TKw(text)'

arm "is_ident answers true for a keyword" \
  'pub fn is_ident(c: Ctx, p: P) -> Bool =
  match kind(c, p) { lexer::TIdent(n) -> true, _ -> false }' \
  'pub fn is_ident(c: Ctx, p: P) -> Bool =
  match kind(c, p) { lexer::TIdent(n) -> true, lexer::TKw(n) -> true, _ -> false }'

# 7. Spans, which every dump record leads with.
arm "span_to takes the first span's start rather than the smaller" \
  '{ start: if a.start < b.start { a.start } else { b.start },' \
  '{ start: a.start,'

arm "span_to takes the first span's end rather than the larger" \
  'end: if a.end > b.end { a.end } else { b.end } }' \
  'end: a.end }'

arm "the primary span is read off the first label, primary or not" \
  'if prim.seen { prim.s } else { first.s }' \
  'first.s'

# 8. The dump encoder's three structural properties.
arm "a list no longer emits its length" \
  'bytes_concat(nlist(len(xs)), bytes_concat_all(map(xs, f)))' \
  'bytes_concat_all(map(xs, f))'

arm "an absent Option and a present one look the same" \
  'pub fn opt(present: Bool) -> Bytes = if present { b"?1;" } else { b"?0;" }' \
  'pub fn opt(present: Bool) -> Bytes = b"?1;"'

arm "a node no longer leads with its span" \
  'bytes_concat_all([num(s.start), b":", num(s.end), b":", tag, b";"])' \
  'bytes_concat_all([tag, b";"])'

arm "a diagnostic drops its secondary labels" \
  'bytes_concat_all(map(d.labels, dump_label)))' \
  'b"")'

arm "a diagnostic drops its note count" \
  'num(len(d.labels)), b":", num(d.notes), b";"' \
  'num(len(d.labels)), b";"'

# 9. Qualified names: the module half is what tells `orders::place` from
#    `place`, and both spell the same span if the qualifier is dropped.
arm "a qualified name loses its module qualifier" \
  '{ p: second.p, node: qualified(first.node, second.node) }' \
  '{ p: second.p, node: bare(second.node) }'

arm "a name with two coloncolons is accepted rather than reported" \
  'else if at(c, second.p, t_coloncolon()) {' \
  'else if false {'

# 10. Depth, which is the one thing standing between the corpus and ADR 0022 §8.
arm "deeper never bails, however deep the nesting" \
  'if q.depth <= max_depth() { q }' \
  'if true { q }'

# 11. `starts_upper`, which decides constructor-versus-binder at every bare name
#     in a pattern and every bare name in a type.
arm "starts_upper answers true for a lowercase name" \
  'bytes_len(name) > 0 && bytes_at(name, 0) >= 65 && bytes_at(name, 0) <= 90' \
  'bytes_len(name) > 0 && bytes_at(name, 0) >= 65'

arm "starts_upper reads past the end of an empty name" \
  'bytes_len(name) > 0 && bytes_at(name, 0) >= 65 && bytes_at(name, 0) <= 90' \
  'bytes_at(name, 0) >= 65 && bytes_at(name, 0) <= 90'

arm "is_str answers true for a byte string as well as a string" \
  'match kind(c, p) { lexer::TStr(v) -> true, _ -> false }' \
  'match kind(c, p) { lexer::TStr(v) -> true, lexer::TBytes(v) -> true, _ -> false }'

arm "dump_bool spells both booleans the same" \
  'pub fn dump_bool(b: Bool) -> Bytes = if b { word(b"true") } else { word(b"false") }' \
  'pub fn dump_bool(b: Bool) -> Bytes = word(b"true")'

arm "bump answers the state before the token, not after" \
  'pub fn bump(c: Ctx, p: P) -> P = advance(c, p).p' \
  'pub fn bump(c: Ctx, p: P) -> P = p'

restore
echo
if [ "$fails" -eq 0 ]; then
  echo "every mutation behaved as registered; the spine's tests are armed"
else
  echo "$fails mutation(s) did not"
fi
"$ply" test "$work" --no-cache >/dev/null 2>&1 || { echo "restore failed: the suite is red"; exit 1; }
exit "$fails"
