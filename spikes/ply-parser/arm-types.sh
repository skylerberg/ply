#!/usr/bin/env bash
# Every green in `types.ply` and `patterns.ply`, seen to go red.
#
# `CONTRIBUTING.md`'s first-named defect is a green result over unexplored
# space, and both files passed every one of their tests on the first run, which
# is exactly when that defect is invisible. This script corrupts one thing at a
# time and asserts the suite fails.
#
# `equiv` is the other half: a mutation that is *supposed* to leave the suite
# green because the mutant is semantically equal to the original. Calling an
# equivalent mutant a hole is how a suite grows tests that assert an
# implementation detail. The three `equiv` entries at the foot are the area's
# most useful result and are written up in `GAPS-types.md` §P7: they are the
# `bail` guards, which the pre-registration expected to be armed by an error
# fixture, and which nothing here can arm because the dedup rule in
# `push_diag` already absorbs what they suppress.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }

work="$(mktemp -d)"
cp "$here/lexer.ply" "$here/spine.ply" "$here/types.ply" "$here/patterns.ply" "$work/"
cp "$work/types.ply" "$work/types.orig"
cp "$work/patterns.ply" "$work/patterns.orig"
restore() { cp "$work/types.orig" "$work/types.ply"; cp "$work/patterns.orig" "$work/patterns.ply"; }
trap 'rm -rf "$work"' EXIT

fails=0
mutate() {
  restore
  perl -0pi -e "s/\Q$2\E/$3/" "$work/$1.ply" || return 1
  grep -qF -- "$3" "$work/$1.ply"
}

arm() {
  local name="$1"; shift
  if ! mutate "$@"; then echo "MUTATION DID NOT LAND: $name"; fails=$((fails + 1)); return; fi
  if "$ply" test "$work" --no-cache >/dev/null 2>&1; then
    echo "NOT ARMED: $name -- the suite stayed green"; fails=$((fails + 1))
  else
    echo "armed:    $name"
  fi
}

equiv() {
  local name="$1"; shift
  if ! mutate "$@"; then echo "MUTATION DID NOT LAND: $name"; fails=$((fails + 1)); return; fi
  if "$ply" test "$work" --no-cache >/dev/null 2>&1; then
    echo "equiv:    $name"
  else
    echo "NO LONGER EQUIVALENT: $name -- the suite went red"; fails=$((fails + 1))
  fi
}

echo "==> the suite is green before any mutation"
restore
"$ply" test "$work" --no-cache >/dev/null 2>&1 || { echo "the suite is RED to begin with"; exit 1; }

echo
echo "== the dump carries what the tree carries =="

arm "a type variable's name is not emitted" types \
  'TVar(i) -> bytes_concat(rec(i.span, b"tvar"), dump_ident(i)),' \
  'TVar(i) -> rec(i.span, b"tvar"),'

arm "a constructor's argument list is not emitted" types \
  'TCon(x) -> bytes_concat_all([rec(x.span, b"tcon"), dump_qname(x.name),
                                 dump_list(x.args, dump_ty)]),' \
  'TCon(x) -> bytes_concat_all([rec(x.span, b"tcon"), dump_qname(x.name)]),'

arm "a function type's effect row is not emitted" types \
  'dump_ty(x.ret), dump_opt(x.effects, dump_row)]),' \
  'dump_ty(x.ret)]),'

arm "read and write are swapped" types \
  'MRead -> word(b"read"), MWrite -> word(b"write")' \
  'MRead -> word(b"write"), MWrite -> word(b"read")'

arm "an atom's resource is not emitted" types \
  'dump_qname(a.eff), dump_mode(a.mode),
                    dump_opt(a.resource, dump_ident)])' \
  'dump_qname(a.eff), dump_mode(a.mode)])'

arm "a row's atoms and aliases are swapped" types \
  'dump_list(r.atoms, dump_atom),
                    dump_list(r.aliases, dump_qname)' \
  'dump_list(r.aliases, dump_qname),
                    dump_list(r.atoms, dump_atom)'

arm "a list pattern's rest binding is not emitted" patterns \
  'PList(v) -> bytes_concat_all([rec(v.span, b"plst"), dump_list(v.items, dump_pattern),
                                  dump_opt(v.rest, dump_pattern)]),' \
  'PList(v) -> bytes_concat_all([rec(v.span, b"plst"), dump_list(v.items, dump_pattern)]),'

arm "a record pattern's rest marker is not emitted" patterns \
  'PRecord(v) -> bytes_concat_all([rec(v.span, b"prec"), opt(v.rest),' \
  'PRecord(v) -> bytes_concat_all([rec(v.span, b"prec"),'

echo
echo "== the parse itself =="

arm "a lowercase bare name in a type becomes a constructor" types \
  'else if is_bare(q.node) && !starts_upper(q.node.name.name) {
    Ok({ p: q.p, node: TVar(q.node.name) })' \
  'else if false {
    Ok({ p: q.p, node: TVar(q.node.name) })'

arm "an uppercase bare name in a pattern becomes a binder" patterns \
  'else if is_bare(q.node) && !starts_upper(q.node.name.name) {' \
  'else if is_bare(q.node) {'

arm "a \`.\` no longer distinguishes an atom from an effect set" types \
  'if !at(c, p, t_dot()) { Ok({ p: p, node: RMSet(q) }) }' \
  'if true { Ok({ p: p, node: RMSet(q) }) }'

arm "the type parameter list no longer stops at \`|\`" types \
  '|| (stop_on_pipe && at(c, s.p, t_pipe()))' \
  '|| (false && at(c, s.p, t_pipe()))'

arm "an atom ends at the current token rather than the previous one" types \
  'span: span_to(ef.span, prev_span(c, p))' \
  'span: span_to(ef.span, cur_span(c, p))'

arm "a parenthesised pattern keeps the inner node's span" patterns \
  'Ok({ p: cl.p, node: set_pat_span(inner.node, span_to(o.node, cl.node)) })' \
  'Ok({ p: cl.p, node: inner.node })'

arm "a negative integer pattern is not negated" patterns \
  'TInt(v) -> Ok({ p: a.p, node: PLit({ span: s, lit: LInt(0 - v) }) }),' \
  'TInt(v) -> Ok({ p: a.p, node: PLit({ span: s, lit: LInt(v) }) }),'

arm "a negative float pattern is sliced over the number token, not the pattern" patterns \
  'TFloat(f) -> Ok({ p: a.p, node: PLit({ span: s, lit: LFloat(src_over(c, s)) }) }),' \
  'TFloat(f) -> Ok({ p: a.p, node: PLit({ span: s, lit: LFloat(src_over(c, span_to(a.node, a.node))) }) }),'

arm "the record-pattern shorthand binds with the whole record's span" patterns \
  'else { Ok({ p: co.p, node: PVar({ span: n.node.span, name: n.node }) }) };' \
  'else { Ok({ p: co.p, node: PVar({ span: open, name: n.node }) }) };'

arm "a \`..\` with a name binds a wildcard instead" patterns \
  'Ok(i) -> Ok({ p: i.p, node: PVar({ span: i.node.span, name: i.node }) }),' \
  'Ok(i) -> Ok({ p: i.p, node: PWild({ span: i.node.span }) }),'

echo
echo "== the diagnostics =="

arm "the no-tuple-type diagnostic loses its note" types \
  'diag1(unexpected(), span_to(o.node, cl.node), 1)' \
  'diag1(unexpected(), span_to(o.node, cl.node), 0)'

arm "an unclosed record type loses the label on its opening brace" types \
  'let {p, node: cl} = expect_close(c, p, t_rbrace(), o.node, b"`}`")?;' \
  'let {p, node: cl} = expect(c, p, t_rbrace(), b"`}`")?;'

arm "an unclosed list pattern loses the label on its opening bracket" patterns \
  'let {p, node: cl} = expect_close(c, p, t_rbracket(), o.node, b"`]` to close the list pattern")?;' \
  'let {p, node: cl} = expect(c, p, t_rbracket(), b"`]` to close the list pattern")?;'

echo
echo "== the bail guards: withdrawn, because there are none =="
echo "   > **Withdrawn (ADR 0028, 2026-08-30).** Four mutations stood here"
echo "   > under two headings, \`== the bail guards: 6 of 50 are killable ==\`"
echo "   > and \`== equivalent mutants: the other 44 ==\`. One was an \`arm\`"
echo "   > (the guard on \`ty_record\`s close result) and three were"
echo "   > \`equiv\`s (the guards at the top of \`ty\` and \`pattern\`, and the"
echo "   > call-site guard in \`ty_field\`), each replacing an \`if p.bail\`"
echo "   > with \`if false\`. The block said: \"Registered arming instrument 4"
echo "   > was delete one \\\`if p.bail\\\` guard and confirm the error fixtures"
echo "   > go red. These three do not, and the reason is measured rather than"
echo "   > guessed: see GAPS-types.md \u00a7P7.\""
echo "   >"
echo "   > \`?\` removed the flag, so there is no guard to delete in either"
echo "   > file. GAPS-types.md \u00a7P7s 0-of-10 is a finding about a design"
echo "   > this area no longer has."

restore
echo
if [ "$fails" -eq 0 ]; then
  echo "every mutation behaved as declared"
else
  echo "$fails mutation(s) did not"
fi
exit "$fails"
