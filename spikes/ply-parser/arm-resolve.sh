#!/usr/bin/env bash
# The resolve differential, armed: each mutation below is applied to a copy of
# `resolve.ply` and the fast half of `harness/tests/resolve.rs` (the hand-written
# programs, the reference's own programs, the standard library) must go red.
# A mutation that stays green watched nothing, and says so.
#
#   ./spikes/ply-parser/arm-resolve.sh
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }
work="$(mktemp -d)"
cp "$here"/{lexer,spine,types,patterns,exprs,items,resolve}.ply "$work/"
cp "$work/resolve.ply" "$work/resolve.orig"
restore() { cp "$work/resolve.orig" "$work/resolve.ply"; }
trap 'rm -rf "$work"' EXIT
fails=0

run_suite() {
  ( cd "$here/harness" \
    && PLY_BIN="$ply" PLY_PARSER_SRC="$work" \
       cargo test --offline --test resolve -- --test-threads=2 \
         the_ply_resolver_agrees_with_ply_syntax_on_the_hand_written_programs \
         the_ply_resolver_agrees_with_ply_syntax_on_the_references_own_programs \
         the_ply_resolver_agrees_with_ply_syntax_on_the_standard_library 2>&1 )
}

mutate() {
  restore
  perl -0pi -e "s/\Q$1\E/$2/" "$work/resolve.ply" || return 1
  # The file must actually differ: the replacement text may already occur elsewhere.
  ! cmp -s "$work/resolve.orig" "$work/resolve.ply"
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
echo "==> declarations and scopes"
arm "a later declaration of the same name wins" \
  '    Some(_) -> d,
    None -> with_space(d, ns, push(space(d, ns),' \
  '    Some(_) -> with_space(d, ns, push(space(d, ns),
                { name: name.name, qualified: qualify(module, name.name),
                  public: is_public(vis), span: name.span })),
    None -> with_space(d, ns, push(space(d, ns),'
arm "a sum type's constructors are declared with the type's privacy inverted" \
  'TDSum(vs) -> fold(vs, d2, |acc: Decls, v: VariantDef| declare(acc, NValue, m.name, v.name, t.vis)),' \
  'TDSum(vs) -> fold(vs, d2, |acc: Decls, v: VariantDef| declare(acc, NValue, m.name, v.name, VPub)),'
arm "a selective import is recorded at the whole import rather than its path" \
  '            None -> push(b2.scope.selective, { binder: key, target: t, span: path_span(d) }),' \
  '            None -> push(b2.scope.selective, { binder: key, target: t, span: d.span }),'
arm "a private name is imported anyway" \
  '    if len(public) == 0 {' \
  '    if false {'
arm "a local definition silently loses to an import" \
  '    Some(prev) -> {..b, diags: push(b.diags, diag2(b"E0108", i, name.span, i, prev.span, 2))},' \
  '    Some(prev) -> b,'
arm "an unknown module is reported at the whole import" \
  '    None -> { b: {..b, diags: push(b.diags, diag1(b"E0106", i, path_span(d), 1))}, t: None },' \
  '    None -> { b: {..b, diags: push(b.diags, diag1(b"E0106", i, d.span, 1))}, t: None },'

echo
echo "==> the order"
arm "a module is ordered before what it imports" \
  '          path: take(d.path, len(d.path) - 1), order: push(d.order, top.v)}' \
  '          path: take(d.path, len(d.path) - 1), order: fold(d.order, [top.v], |acc: List<Int>, x: Int| push(acc, x))}'
arm "a module importing itself is reported as a longer cycle" \
  '  if len(c.nodes) == 1 { diag1(b"E0109", c.closing.from, c.closing.span, 1) }' \
  '  if len(c.nodes) == 0 { diag1(b"E0109", c.closing.from, c.closing.span, 1) }'

echo
echo "==> defaults"
arm "a call in a default is admissible" \
  '    EApp(a) -> len(a.named) == 0
               && (match a.func { EVar(v) -> is_ctor_name(v.name.name.name), _ -> false })
               && all_default(a.args),' \
  '    EApp(a) -> len(a.named) == 0 && all_default(a.args),'
arm "a spliced default is not qualified against the module that wrote it" \
  'name: { module: Some({ name: module, span: v.name.span }),' \
  'name: { module: None,'
arm "a named argument fills the first empty slot rather than its own" \
  '          None -> {..acc, slots: set_at(acc.slots, i, Some(n.value))},' \
  '          None -> {..acc, slots: set_at(acc.slots, 0, Some(n.value))},'
arm "a call that leaves a parameter unfilled is reported without its note" \
  'diag1(b"E0125", cx.module, a.span, 1)' \
  'diag1(b"E0125", cx.module, a.span, 0)'
arm "a local binder no longer shadows a function with defaults" \
  '      else if is_bare(v.name) && contains_bytes(cx.scope, v.name.name.name) { None }' \
  '      else if false { None }'

echo
if [ "$fails" -eq 0 ]; then echo "all mutations behaved as declared"; else echo "$fails mutation(s) did not"; exit 1; fi
