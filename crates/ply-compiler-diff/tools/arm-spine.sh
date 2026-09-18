#!/usr/bin/env bash
# Mutates spine.ply one thing at a time: every `arm` must turn the suite red, every
# `equiv` (a semantically equal mutant) must leave it green.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
src="$root/crates/ply-compiler/ply"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }

# A private copy: ply test typechecks every module in a directory, so a half-written
# neighbour in crates/ply-compiler would redden the suite for the wrong reason.
work="$(mktemp -d)"
cp "$src/lexer.ply" "$src/spine.ply" "$work/"
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

arm "gt_split: the split token is never applied on read" \
  'if j == p.gt_split {' \
  'if false {'

arm "gt_split: the split = keeps the whole >= span" \
  '{ start: t.start + 1, end: t.end, tok: lexer::TPunct(b"eq") }' \
  '{ start: t.start, end: t.end, tok: lexer::TPunct(b"eq") }'

arm "expect_gt answers the whole >= as the >" \
  'node: {start: s.start, end: s.start + 1} }' \
  'node: {start: s.start, end: s.end} }'

arm "dedup rule: every diagnostic is kept" \
  'if dup { p } else {' \
  'if false { p } else {'

arm "dedup rule: keyed on the code alone, not the span" \
  'l.code == d.code && ls.start == s.start && ls.end == s.end' \
  'l.code == d.code'

arm "the lexer's diagnostics do not seed the dedup key" \
  'match list_at(p.diags, len(p.diags) - 1) {' \
  'match list_at(p.diags, len(p.diags)) {'

arm "comma_list: a failing element is swallowed rather than propagated" \
  'Err(q) -> Stop(Err(q)),
        Ok(r) -> {' \
  'Err(q) -> Stop(Ok({p: q, node: s.out})),
        Ok(r) -> {'

arm "comma_list: the last element before the closer is dropped" \
  'Stop(Ok({p: e.p, node: push(s.out, r.node)}))' \
  'Stop(Ok({p: e.p, node: s.out}))'

arm "comma_list: end of input is not a stop condition" \
  'if at(c, s.p, close) || at_eof(c, s.p) {' \
  'if at(c, s.p, close) {'

arm "comma_list: a missing comma does not end the list" \
  'let e = eat(c, r.p, t_comma());
          if e.ok {' \
  'let e = eat(c, r.p, t_comma());
          if true {'

# Token access, where an off-by-one is invisible to a span-blind comparator.
arm "advance runs off the end of the buffer at EOF" \
  'if p.pos + 1 < c.ntok { at_pos(p, p.pos + 1) } else { p }' \
  'at_pos(p, p.pos + 1)'

arm "prev_span does not saturate at position zero" \
  'let j = if i < 0 { 0 } else if i > c.ntok - 1 { c.ntok - 1 } else { i };' \
  'let j = if i > c.ntok - 1 { c.ntok - 1 } else { i };'

arm "lookahead past the end does not clamp" \
  'else if i > c.ntok - 1 { c.ntok - 1 } else { i };' \
  'else { i };'

# Contextual keywords stay usable as names, so a keyword must never match as an identifier.
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

arm "span_to takes the first span's start rather than the smaller" \
  '{ start: if a.start < b.start { a.start } else { b.start },' \
  '{ start: a.start,'

arm "span_to takes the first span's end rather than the larger" \
  'end: if a.end > b.end { a.end } else { b.end } }' \
  'end: a.end }'

arm "the primary span is read off the first label, primary or not" \
  'if prim.seen { prim.s } else { first.s }' \
  'first.s'

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

arm "a qualified name loses its module qualifier" \
  'Ok({ p: p, node: qualified(first, second) })' \
  'Ok({ p: p, node: bare(second) })'

arm "a name with two coloncolons is accepted rather than reported" \
  'if at(c, p, t_coloncolon()) {
      Err(push_diag' \
  'if false {
      Err(push_diag'

arm "deeper never fails, however deep the nesting" \
  'if q.depth <= max_depth() { Ok(q) }' \
  'if true { Ok(q) }'

# starts_upper decides constructor-versus-binder at every bare name in a pattern or type.
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
