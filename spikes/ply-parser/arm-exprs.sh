#!/usr/bin/env bash
# Every green in `exprs.ply`, seen to go red.
#
# `CONTRIBUTING.md`'s first-named defect is a green result over unexplored
# space, and this file passed its whole suite on the first run after the
# expectations were corrected — which is exactly when that defect is invisible.
# Each `arm` corrupts one thing and asserts the suite fails.
#
# `equiv` is the other half, and in this area it is the more interesting one: a
# mutation that is *supposed* to leave the suite green because the mutant is
# semantically equal. The `bail` guards below are equivalent mutants. A sweep
# that deleted each of the 41 guards in the four modules one at a time found
# **18 individually detected and 23 not** — not because the tests are weak but
# because a deleted guard is caught by the guard of the first function the
# unguarded body calls. That number is the price of the design decision that
# replaces `?`, and it is written up in `GAPS-exprs.md` §4.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }

work="$(mktemp -d)"
cp "$here/lexer.ply" "$here/spine.ply" "$here/types.ply" "$here/patterns.ply" \
   "$here/exprs.ply" "$work/"
cp "$work/exprs.ply" "$work/exprs.orig"
restore() { cp "$work/exprs.orig" "$work/exprs.ply"; }
trap 'rm -rf "$work"' EXIT

fails=0
mutate() {
  restore
  perl -0pi -e "s/\Q$1\E/$2/" "$work/exprs.ply" || return 1
  grep -qF -- "$2" "$work/exprs.ply"
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

echo "==> green before any mutation"
restore
"$ply" test "$work" --no-cache >/dev/null 2>&1 || { echo "the suite is RED before any mutation"; exit 1; }
echo "ok"
echo
echo "==> the precedence table"
arm "* and + swap binding powers" \
    'Some({op: b"mul", bp: 6})' 'Some({op: b"mul", bp: 5})'
arm "the right operand is parsed at bp, so the operators right-associate" \
    'bin_expr(c, a.p, o.bp + 1)' 'bin_expr(c, a.p, o.bp)'
arm "a binding power below the minimum no longer stops the loop" \
    'if o.bp < min_bp { Stop(Ok(s)) } else {' 'if false { Stop(Ok(s)) } else {'

echo
echo "==> no_brace, the flag eight of this area's functions save and restore"
arm "the scrutinee does not set no_brace" \
    'match expr(c, with_no_brace(p, true)) {' 'match expr(c, p) {'
arm "an argument list does not clear no_brace" \
    'match call_args_inner(c, with_no_brace(p, false)) {' 'match call_args_inner(c, p) {'
arm "simulate ignores no_brace and eats the arm block" \
    'name == b"simulate" && !p.no_brace && kind_at(c, p, 1) == t_lbrace()' 'name == b"simulate" && kind_at(c, p, 1) == t_lbrace()'

echo
echo "==> the decisions two tokens of lookahead make"
arm "a perform no longer requires an argument list after the operation" \
    'TIdent(n) -> at(c, s.p, t_dot()) && follows_op(kind_at(c, s.p, 2)),' 'TIdent(n) -> at(c, s.p, t_dot()),'
arm "a record literal is no longer recognised by its comma" \
    'kind_at(c, p, 2) == t_colon() || kind_at(c, p, 2) == t_comma()' 'kind_at(c, p, 2) == t_colon()'
arm "resume stops being a contextual keyword" \
    'if !at_ident_text(c, p, b"resume") { Ok({ p: p, node: None }) } else {' 'if true { Ok({ p: p, node: None }) } else {'

echo
echo "==> spans, which a span-blind comparator would not see at all"
arm "a parenthesized expression keeps its inner span" \
    'Ok({ p: cl.p, node: set_span(inner.node, span_to(o.node, cl.node)) })' 'Ok({ p: cl.p, node: inner.node })'
arm "the synthesized else takes a dummy span instead of the block end" \
    'node: ELit({ span: {start: end, end: end}, lit: LUnit }) }' 'node: ELit({ span: dummy_span(), lit: LUnit }) }'

echo
echo "==> statements, arms and clauses"
arm "an if no longer counts as block-like, so it needs a semicolon" \
    'EIf(v) -> true,
    EMatch(v) -> true,' 'EIf(v) -> false,
    EMatch(v) -> true,'
arm "a block's last expression becomes a statement rather than the tail" \
    'Continue({p: semi.p, tail: Some(e.node), stmts: s.stmts})' 'Continue({p: semi.p, tail: None, stmts: push(s.stmts, SExpr(e.node))})'
arm "a second return clause is accepted silently" \
    '{p: push_diag(rc.p, diag2(unexpected(), rc.node.span, prev.span, 0)),
             clauses: s.clauses, ret: s.ret},' '{p: rc.p, clauses: s.clauses, ret: Some(rc.node)},'
arm "a match arm no longer needs a comma" \
    'if comma.ok || at(c, comma.p, t_rbrace()) {' 'if true {'
arm "unary_expr stops counting depth, so MAX_DEPTH is never reached" \
    'let d = deeper(c, p)?;
  match unary_body(c, d) {' 'let d = p;
  match unary_body(c, d) {'

echo
echo "==> the bail guards: withdrawn, because there are none"
echo "   > **Withdrawn (the try operator, 2026-08-30).** Seven mutations stood here in"
echo "   > two blocks. Four were \`equiv\`s under \"equivalent mutants: the bail"
echo "   > guards this suite cannot arm, and why\" -- \`bin_expr\`,"
echo "   > \`postfix_expr\`, \`block_expr\`, \`record_field\` -- and three were"
echo "   > \`arm\`s under \"and the guards that are NOT equivalent, to show the"
echo "   > distinction is real\" -- \`primary_expr\`, \`if_expr\`, \`let_stmt\`."
echo "   > Each replaced \`if p.bail {\` with \`if false {\` at the head of one"
echo "   > function."
echo "   >"
echo "   > This module had 27 guards, the most of the five, and GAPS-exprs.md"
echo "   > \u00a74 recorded that ten of its functions violated the invariant on the"
echo "   > first write. There is no flag now: \`?\` propagates the failure and"
echo "   > no function can be entered after one. The three ARMED guards above"
echo "   > were the strongest evidence in the spike that the discipline was"
echo "   > load-bearing; they are also the clearest thing \`?\` deletes."

restore
echo
if [ "$fails" -eq 0 ]; then echo "all mutations behaved as declared"; else echo "$fails did not"; fi
exit "$fails"
