#!/usr/bin/env bash
# The rewrite differential, armed: each mutation below is applied to a copy of
# `rewrite.ply` and the fast half of `harness/tests/rewrite.rs` (the hand-written
# fixtures and the reference's own inputs) must go red.
#
#   ./spikes/ply-parser/arm-rewrite.sh
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }
work="$(mktemp -d)"
cp "$here"/{lexer,spine,types,patterns,exprs,items,rewrite}.ply "$work/"
cp "$work/rewrite.ply" "$work/rewrite.orig"
restore() { cp "$work/rewrite.orig" "$work/rewrite.ply"; }
trap 'rm -rf "$work"' EXIT
fails=0

run_suite() {
  ( cd "$here/harness" \
    && PLY_BIN="$ply" PLY_PARSER_SRC="$work" \
       cargo test --offline --test rewrite -- --test-threads=2 \
         the_rewrites_agree_with_ply_syntax_on_the_hand_written_fixtures \
         the_rewrites_agree_with_ply_syntax_on_the_reference_own_test_inputs 2>&1 )
}

mutate() {
  restore
  perl -0pi -e "s/\Q$1\E/$2/" "$work/rewrite.ply" || return 1
  ! cmp -s "$work/rewrite.orig" "$work/rewrite.ply"
}

arm() {
  local name="$1"; shift
  if ! mutate "$@"; then echo "MUTATION DID NOT LAND: $name"; fails=$((fails + 1)); return; fi
  local out
  out=$(run_suite)
  if [[ "$out" == *"test result: ok"* ]]; then
    echo "NOT ARMED: $name -- the differential stayed green"; fails=$((fails + 1))
  elif [[ "$out" == *"disagree"* ]]; then
    echo "armed:    $name"
  else
    echo "INVALID:  $name -- the mutant did not run:"; printf '%s\n' "$out" | grep -E '^error|panicked at' | head -3
    fails=$((fails + 1))
  fi
}

echo "==> green before any mutation"
restore
out=$(run_suite)
[[ "$out" == *"test result: ok"* ]] || { echo "the differential is RED before any mutation"; printf '%s\n' "$out" | tail -20; exit 1; }
echo "ok"

echo
echo "==> effect sets"
arm "an expansion keeps its duplicates" \
  '    if len(acc) > 0 && atom_same(at(acc, len(acc) - 1), x) { acc } else { push(acc, x) })' \
  '    push(acc, x))'
arm "a cycle is reported with one note" \
  '  { code: b"E0115", notes: 2,' \
  '  { code: b"E0115", notes: 1,'
arm "a set named from another module is reported with one note" \
  '    { s: {..s, diags: push(s.diags, diag1(b"E0114", q.span, 2))}, at: None }' \
  '    { s: {..s, diags: push(s.diags, diag1(b"E0114", q.span, 1))}, at: None }'
arm "a row keeps only its written atoms" \
  '  { s: looked.s, r: { atoms: append(r.atoms, looked.atoms), aliases: r.aliases, tail: r.tail, span: r.span } }' \
  '  { s: looked.s, r: r }'

echo
echo "==> record update"
arm "the copied fields keep declaration order" \
  '        let sorted = fold(copies, [], |acc: List<Bytes>, n: Bytes| insert_bytes(acc, n));' \
  '        let sorted = copies;'
arm "a copied field takes the span of the whole update" \
  '          let field = { name: n, span: base_span };' \
  '          let field = { name: n, span: span };'
arm "a let's written type is not read" \
  '              PVar(n) -> match l.ty { Some(t) -> ru_bind(v.cx, n.name, Some(t)), None -> ru_bind_pattern(v.cx, l.pat) },' \
  '              PVar(n) -> ru_bind_pattern(v.cx, l.pat),'
arm "an unknown written field is not reported" \
  '        else { { cx: {..acc.cx, diags: push(acc.cx.diags, diag2(b"E0117", w.name.span, base_span, 1))}, bad: true } });' \
  '        else { { cx: acc.cx, bad: true } });'

echo
echo "==> the try operator"
arm "the success arm comes first" \
  '                    arms: [{ pat: fail_pat, guard: None, body: fail_body, span: at_ },
                           { pat: ctor_pattern(b"Ok", [pat], at_), guard: None, body: body, span: at_ }] }) }' \
  '                    arms: [{ pat: ctor_pattern(b"Ok", [pat], at_), guard: None, body: body, span: at_ },
                           { pat: fail_pat, guard: None, body: fail_body, span: at_ }] }) }'
arm "a refused `?` keeps the wrong code" \
  '      let code = if scope || barrier { b"E0118" } else { b"E0119" };' \
  '      let code = b"E0118";'
arm "an impure operand before a `?` is lifted anyway" \
  '      match o.s { SFound(_) -> { cx: o.cx, s: o.s, e: rebuilt }, _ -> { cx: o.cx, s: SImpure, e: rebuilt } }
    },
    EPerform(p) -> {' \
  '      match o.s { SFound(_) -> { cx: o.cx, s: o.s, e: rebuilt }, _ -> { cx: o.cx, s: SPure, e: rebuilt } }
    },
    EPerform(p) -> {'
arm "a lambda with a written return type is still a barrier" \
  '        Some(ty) -> {
          let mode = if cx.shadowed { None } else { mode_of(cx, ty, 0) };' \
  '        Some(ty) -> {
          let mode = None;'
arm "the fresh binders are numbered per module rather than per item" \
  '  let cx = {..cx0, fresh: 0, mode: None};' \
  '  let cx = {..cx0, mode: None};'

echo
if [ "$fails" -eq 0 ]; then echo "all mutations behaved as declared"; else echo "$fails mutation(s) did not"; exit 1; fi
