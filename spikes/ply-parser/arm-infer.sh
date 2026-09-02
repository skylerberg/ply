#!/usr/bin/env bash
# The checker differential, armed: each mutation below is applied to a copy of
# `infer.ply` or `tycore.ply` and the fast half of `harness/tests/infer.rs` (the
# hand-written programs, the resolver's programs, the standard library and the
# reference checker's own inputs) must go red.
#
#   ./spikes/ply-parser/arm-infer.sh
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }
work="$(mktemp -d)"
cp "$here"/{lexer,spine,types,patterns,exprs,items,rewrite,resolve,derive,tycore,infer}.ply "$work/"
cp "$work/infer.ply" "$work/infer.orig"
cp "$work/tycore.ply" "$work/tycore.orig"
restore() { cp "$work/infer.orig" "$work/infer.ply"; cp "$work/tycore.orig" "$work/tycore.ply"; }
trap 'rm -rf "$work"' EXIT
fails=0

run_suite() {
  ( cd "$here/harness" \
    && PLY_BIN="$ply" PLY_PARSER_SRC="$work" \
       cargo test --offline --test infer -- --test-threads=2 \
         the_ply_checker_agrees_with_ply_core_on_the_resolvers_hand_written_programs \
         the_ply_checker_agrees_with_ply_core_on_the_checkers_hand_written_programs \
         the_ply_checker_agrees_with_ply_core_on_the_resolvers_reference_programs \
         the_ply_checker_agrees_with_ply_core_on_the_standard_library \
         the_ply_checker_agrees_with_ply_core_on_the_references_own_inputs \
         the_ply_checker_restored_from_its_own_interfaces_agrees_with_ply_core_on_the_bundles 2>&1 )
}

mutate() {
  local file="$1"
  restore
  perl -0pi -e "s/\Q$2\E/$3/" "$work/$file.ply" || return 1
  ! cmp -s "$work/$file.orig" "$work/$file.ply"
}

arm() {
  local name="$1"; local file="$2"; shift 2
  if ! mutate "$file" "$@"; then echo "MUTATION DID NOT LAND: $name"; fails=$((fails + 1)); return; fi
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
echo "==> unification and generalization"
arm "a rigid type variable binds like any other" tycore \
  '        _ -> if !is_rigid_ty(s, x) { bind_ty(s, f, x, b) } else { failed(s, f, Mismatch({ expected: a, found: b })) },' \
  '        _ -> bind_ty(s, f, x, b),'
arm "generalization leaves row variables monomorphic" tycore \
  '  { e: env.e, sc: { ty_vars: ints_difference(fv.tys, env.free.tys), row_vars: ints_difference(fv.rows, env.free.rows), ty: ty } }' \
  '  { e: env.e, sc: { ty_vars: ints_difference(fv.tys, env.free.tys), row_vars: [], ty: ty } }'
arm "a closed row absorbs an atom it lacks" tycore \
  '        None -> if len(only_a) == 0 && len(only_b) == 0 { ok(s, f) } else { failed(s, f, bad) },' \
  '        None -> ok(s, f),'
arm "the printer gives every type variable the first letter" tycore \
  '      let name = letter_name(ty_letters(), map_len(p.ty_names));' \
  '      let name = letter_name(ty_letters(), 0);'
echo
echo "==> the checker"
arm "the row of a call is not joined into the row of the caller" infer \
  '        let j: WithRow = join(recorded, span, j0.r, effects);
        { cx: j.cx, ty: fn_.ret, row: j.r }' \
  '        { cx: recorded, ty: fn_.ret, row: j0.r }'
arm "a perform contributes no atom" infer \
  '            let j2: WithRow = join(j1.cx, span, j1.r, row_singleton(atom));
            { cx: j2.cx, ty: shape.ret, row: j2.r }' \
  '            { cx: j1.cx, ty: shape.ret, row: j1.r }'
arm "a handled atom stays in the row" infer \
  '  let remaining = row_without(b.row, walked.handled);' \
  '  let remaining = b.row;'
arm "an unreachable row variable is left open" infer \
  '            if contains_int(all.rows, tail) { cx } else {
              let u = unify_row(cx.subst, cx.fresh, row_empty(), row_open(tail));
              with_unified(cx, u)
            }' \
  '            cx'
arm "the scheme of a definition is published monomorphic" infer \
  '    let g: Generalized = generalize(c.subst, c.env, sig.fn_ty);' \
  '    let g: Generalized = { e: c.env, sc: mono(resolve(c, sig.fn_ty)) };'
arm "the fields of a constructor are declared in reverse" infer \
  '                      let ty = if len(fs.ts) == 0 { result } else { t_fn(fs.ts, result, row_empty()) };' \
  '                      let ty = if len(fs.ts) == 0 { result } else { t_fn(reverse_types(fs.ts), result, row_empty()) };
                      let unused = reverse_types;'
arm "a body that performs more than it declares is not reported" infer \
  '  let c1: Cx = if len(extra) == 0 { cx } else {' \
  '  let c1: Cx = if true { cx } else {'
arm "internal effects are not propagated to callers" infer \
  '          if at(a.effects, caller) { a } else { { effects: set_at(a.effects, caller, true), pending: push(a.pending, caller) } });' \
  '          a);'
arm "the numeric operand type defaults to Int" infer \
  '      TyVar(_) -> err1(c, b"E0210", entry.span, 2),' \
  '      TyVar(_) -> c,'
arm "the footprint of a test drops what its body performs" infer \
  '            let row = resolve_r(popped, o.row);
            { cx: if def.is_nondet { popped } else { check_determinism(popped, def, row.atoms) }, atoms: row.atoms }' \
  '            let row = row_empty();
            { cx: if def.is_nondet { popped } else { check_determinism(popped, def, row.atoms) }, atoms: row.atoms }'

echo
echo "==> the restored path"
arm "a restored definition is published with an empty footprint" infer \
  '    set_def(bound, { name: name, module: m.name, simple_name: def.name.name, scheme: a.sc, footprint: entry.footprint,' \
  '    set_def(bound, { name: name, module: m.name, simple_name: def.name.name, scheme: a.sc, footprint: [],'
arm "a restored definition loses its constraints" infer \
  '        { cx: record_spec_env(c2, def, name, s.sig), cs: recovered_constraints(c2, def, s.sig, inst.args) }' \
  '        { cx: record_spec_env(c2, def, name, s.sig), cs: [] }'
arm "a cached test footprint is dropped" infer \
  '          Some(cached) -> { cx: c, atoms: cached },' \
  '          Some(cached) -> { cx: c, atoms: [] },'

echo
if [ "$fails" -eq 0 ]; then echo "all mutations behaved as declared"; else echo "$fails mutation(s) did not"; exit 1; fi
