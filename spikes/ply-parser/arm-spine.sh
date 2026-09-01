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

# 2. > **Withdrawn (the try operator, 2026-08-30): eight mutations that cannot be
#    > written any more.** This block held six `arm`s and two `equiv`s, all of
#    > the shape *"bail guard deleted from `expect`"* — replacing
#    > `pub fn expect(..) -> R<Span> =\n  if p.bail {` with `if false {` — for
#    > `expect`, `expect_close`, `expect_ident`, `expect_gt`, `eat` and
#    > `deeper`, plus two registered as **equivalent** mutants for `comma_list`
#    > and `qname`. Its header read: *"Rule 1's guard, at each function that
#    > touches a token before its first guarded callee. This failure mode has no
#    > analogue in the lexer spike: GAPS.md §12 records that error accumulation
#    > cost the lexer nothing because a lexer never fails."*
#    >
#    > And `qname`'s carried the finding: *"`qname`'s own guard is an EQUIVALENT
#    > MUTANT and this is the interesting result of the whole script … the
#    > guard-deletion instrument is weaker than a raw count of ~93 guards
#    > suggests: every guard on a function whose first act is a guarded call is
#    > unkillable."*
#    >
#    > **There is no guard to delete.** `?` replaced the flag, so `p.bail` does
#    > not exist, no function opens with one, and a mutation that removes a
#    > guard has nothing to remove. That is the try operator's claim in its strongest
#    > form: the 63-of-83 unverifiable guards `GAPS.md` §2 measured are not now
#    > verifiable, they are **unwritable**. The eight lines are deleted rather
#    > than rewritten because a mutation that cannot be applied arms nothing,
#    > and `arm()` above scores it as a failure — correctly.

# 3. Parser::push's dedup rule, which changes the diagnostic list exactly.
arm "dedup rule: every diagnostic is kept" \
  'if dup { p } else {' \
  'if false { p } else {'

arm "dedup rule: keyed on the code alone, not the span" \
  'l.code == d.code && ls.start == s.start && ls.end == s.end' \
  'l.code == d.code'

# The two fields this used to corrupt are gone (the list index decision): `push_diag` reads
# `list_at(p.diags, len(p.diags) - 1)` instead. Corrupting the *index* is the
# same corruption — it makes the rule look at the wrong diagnostic, which for a
# one-element list is none at all.
arm "the lexer's diagnostics do not seed the dedup key" \
  'match list_at(p.diags, len(p.diags) - 1) {' \
  'match list_at(p.diags, len(p.diags)) {'

# 4. The sequence driver, in the ways it can be subtly wrong.
# The "bailed element is pushed" mutation went with the flag: an element that
# fails is an `Err` the loop cannot look inside, so there is no node to push.
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
  'Ok({ p: p, node: qualified(first, second) })' \
  'Ok({ p: p, node: bare(second) })'

arm "a name with two coloncolons is accepted rather than reported" \
  'if at(c, p, t_coloncolon()) {
      Err(push_diag' \
  'if false {
      Err(push_diag'

# 10. Depth, which is the one thing standing between the corpus and the call-ceiling decision.
arm "deeper never fails, however deep the nesting" \
  'if q.depth <= max_depth() { Ok(q) }' \
  'if true { Ok(q) }'

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
