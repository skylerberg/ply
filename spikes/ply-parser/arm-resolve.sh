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
  grep -qF -- "$2" "$work/resolve.ply"
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
arm "a selective import binds the module name too" \
  '          fold(names, b3, |acc: Builder, n: Ident| bind_name(w, i, acc, t, n))' \
  '          fold(names, bind_module(w, i, b3, d, t), |acc: Builder, n: Ident| bind_name(w, i, acc, t, n))'
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
arm "a cycle is reported twice, once per root that reaches it" \
  '      if contains_key(d.found, key) { {..d, stack: again} } else {' \
  '      if false { {..d, stack: again} } else {'

echo
echo "==> defaults"
arm "a call in a default is admissible" \
  '    EApp(a) -> len(a.named) == 0
               && (match a.func { EVar(v) -> is_ctor_name(v.name.name.name), _ -> false })
               && all_default(a.args),' \
  '    EApp(a) -> len(a.named) == 0 && all_default(a.args),'
arm "a spliced default is not qualified against the module that wrote it" \
  '                e: EVar({ span: v.span,
                          name: { module: Some({ name: module, span: v.name.span }),
                                  name: v.name.name, span: v.name.span } }) }' \
  '                e: e }'
arm "a named argument fills the first empty slot rather than its own" \
  '          None -> {..acc, slots: set_at(acc.slots, i, Some(n.value))},' \
  '          None -> {..acc, slots: set_at(acc.slots, 0, Some(n.value))},'
arm "a call that leaves a parameter unfilled keeps its named arguments" \
  '    { cx: cx2, e: EApp({..a, named: []}) }' \
  '    { cx: cx2, e: EApp(a) }'
arm "a local binder no longer shadows a function with defaults" \
  '      else if is_bare(v.name) && contains_bytes(cx.scope, v.name.name.name) { None }' \
  '      else if false { None }'

echo
if [ "$fails" -eq 0 ]; then echo "all mutations behaved as declared"; else echo "$fails mutation(s) did not"; exit 1; fi
