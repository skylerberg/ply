#!/usr/bin/env bash
#
# Every green in this area is worthless until it has been seen to go red.
# `CONTRIBUTING.md`'s first-named defect is a green result over unexplored
# space, so this file corrupts, one at a time, each thing the area's checks
# claim to watch, and prints which instrument noticed.
#
#   ./spikes/ply-parser/arm-items.sh
#
# There are two instruments and they are not interchangeable, which is itself a
# result:
#
#   TESTS   `ply test` over `items.ply`'s own 13 `test` blocks, which assert an
#           exact dump and therefore pin the **tree**.
#   DIAG    a differential against the shipping parser: for 28 fixtures that
#           reach the error paths, `ply check --json`'s diagnostics against this
#           parser's, compared on code, every label's span, every label's
#           primary flag, and the note count. It pins the **diagnostics** and
#           nothing else.
#
# A mutation that only TESTS catches is a tree difference no diagnostic
# comparison can see. A mutation that neither catches is a line of this parser
# that nothing in the area distinguishes from any other line, and those are
# listed at the end rather than left out.
set -u
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)
ply="$root/target/debug/ply"
[ -x "$ply" ] || ply="$root/target/release/ply"
[ -x "$ply" ] || { echo "no ply binary; build ply-cli first" >&2; exit 2; }
work=${PLY_PARSER_WORKDIR:-/tmp/pitems}
[ -f "$work/items.ply" ] || { echo "no assembled project at $work" >&2; exit 2; }
diff_py="$here/diff-items.py"

orig=$(mktemp); cp "$here/items.ply" "$orig"
restore() { cp "$orig" "$work/items.ply"; }
trap restore EXIT

green_tests() { "$ply" test "$work" --no-cache 2>&1 | grep -q "0 failed"; }
green_diag()  { [ "$(python3 "$diff_py" "$work" 2>&1 | tail -1)" = "32 of 32 agree" ]; }

probe() {                                   # name, old, new
  local name="$1" old="$2" new="$3"
  restore
  python3 - "$work/items.ply" "$old" "$new" <<'PY' || { printf '  %-46s MUTATION MISSED\n' "$1"; return; }
import sys
p, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(p).read()
if s.count(old) != 1:
    sys.exit(1)
open(p, 'w').write(s.replace(old, new, 1))
PY
  local t d caught
  green_tests && t=green || t=RED
  green_diag  && d=green || d=RED
  if   [ "$t$d" = "greengreen" ]; then caught="NEITHER -- nothing here distinguishes this line"
  elif [ "$t" = RED ] && [ "$d" = RED ]; then caught="tests + differential"
  elif [ "$t" = RED ]; then caught="tests only  (a tree difference the diagnostics cannot see)"
  else caught="differential only"
  fi
  printf '  %-46s %s\n' "$name" "$caught"
}

echo "Arming the Items area. Each line corrupts one thing and reports what noticed."
echo

probe "recover_to_item's already-at-an-item-start test" \
  'let p0 = if !at_eof(c, p) && !at_item_start(c, p) { bump(c, p) } else { p };' \
  'let p0 = bump(c, p);'

probe "recover_to_item's bracket-depth counter" \
  'let d = if is_open(k) { s.depth + 1 }' \
  'let d = if is_open(k) { s.depth }'

probe "the out-of-order import loses its secondary label" \
  'Some(i) -> push_diag(p, diag2(unexpected(), cur_span(c, p), item_span(i), 1)),' \
  'Some(i) -> push_diag(p, diag1(unexpected(), cur_span(c, p), 1)),'

probe "the pub-on-a-test diagnostic loses its note" \
  'let d = test_def(c, no_pub(p, unexpected(), pub_span, 1))?;' \
  'let d = test_def(c, no_pub(p, unexpected(), pub_span, 0))?;'

probe "the deriver table accepts any name" \
  '  else if name == b"ord" { Some(DOrd) }
  else { None }' \
  '  else if name == b"ord" { Some(DOrd) }
  else { Some(DJson) }'

probe "an effect set may carry a row variable" \
  'if at(c, ms.p, t_pipe()) {
    // A set denotes' \
  'if false {
    // A set denotes'

probe "looks_like_variants treats every name as a sum" \
  'starts_upper(n) && (kind_at(c, p, 1) == t_lparen() || kind_at(c, p, 1) == t_pipe()),' \
  'starts_upper(n),'

probe 'at_law_start drops its quoted-label lookahead' \
  'at_ident_text(c, p, b"law")
    && (tok_is_str(kind_at(c, p, 1))' \
  'at_ident_text(c, p, b"law")
    && (true || tok_is_str(kind_at(c, p, 1))'

probe "an effect set is recognised without its third token" \
  'pub fn at_effect_set_start(c: Ctx, p: P) -> Bool =
  at(c, p, k_effect())' \
  'pub fn at_effect_set_start(c: Ctx, p: P) -> Bool =
  false && at(c, p, k_effect())'

# The anchor here named `body.node`, which stopped existing when `?` replaced
# the `bail` flag and `fn_def` began destructuring its callee's answer
# (`let {p, node: body} = fn_body(c, p)?`). It reported MUTATION MISSED from
# that day until 2026-08-30 -- a corruption that tests nothing and says so, but
# only to a reader of the last column. Re-anchored, not dropped: it is the only
# probe in this file aimed at an item's span.
probe 'a FnDef span stops at its own keyword' \
  'body: body, span: span_to(st.node, expr_span(body)) } })' \
  'body: body, span: st.node } })'

probe "an import kind reports names and alias interchangeably" \
  'INames(ns) -> at_ident_text(c, p, b"as"),' \
  'INames(ns) -> false,'

probe "op_param eats the documentation name it should skip" \
  'let p2 = if is_ident(c, p) && kind_at(c, p, 1) == t_colon() {
             bump(c, bump(c, p))
           } else { p };' \
  'let p2 = p;'

# > **Withdrawn (ADR 0028, 2026-08-30): three probes that cannot be applied.**
# > They stood here as `probe "the bail guard on fn_body is deleted"`,
# > `.. on law_def ..` and `.. on item ..`, each replacing `if p.bail {` at the
# > head of the function with `if false {`. `fn_body`'s is the one `GAPS.md` §2
# > quotes at length as **the** load-bearing guard in the whole parser: with it
# > gone, `at(c, e.p, t_lbrace())` answered `false` on a bailed state and a
# > phantom `E0001` appeared that the reference never raises.
# >
# > `?` deletes the shape rather than the guard. `where_clause`'s failure now
# > leaves `fn_def` at its `?`, so `fn_body` is never entered and there is no
# > `if p.bail` anywhere to turn into `if false`. See the note above `fn_body`
# > in `items.ply`.

restore
echo
echo -n "restored: "
green_tests && echo -n "tests green, " || echo -n "TESTS STILL RED, "
green_diag  && echo "differential green" || echo "DIFFERENTIAL STILL RED"
