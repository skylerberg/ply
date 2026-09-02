#!/usr/bin/env bash
# The derive differential, armed: each mutation below is applied to a copy of
# `derive.ply` and `harness/tests/derive.rs` must go red.
#
#   ./spikes/ply-parser/arm-derive.sh
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }
work="$(mktemp -d)"
cp "$here"/{lexer,spine,types,patterns,exprs,items,rewrite,resolve,derive}.ply "$work/"
cp "$work/derive.ply" "$work/derive.orig"
restore() { cp "$work/derive.orig" "$work/derive.ply"; }
trap 'rm -rf "$work"' EXIT
fails=0

run_suite() {
  ( cd "$here/harness" \
    && PLY_BIN="$ply" PLY_PARSER_SRC="$work" \
       cargo test --offline --test derive -- --test-threads=2 2>&1 )
}

# The pattern is quoted literally; the replacement is perl's, so a `\"` in it is written `\\"`.
mutate() {
  restore
  perl -0pi -e "s/\Q$1\E/$2/" "$work/derive.ply" || return 1
  ! cmp -s "$work/derive.orig" "$work/derive.ply"
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
echo "==> the rules"
arm "snake_case never inserts an underscore" \
  '    bytes_concat_all([out, if sep { b"_" } else { b"" }, byte_of(lower_byte(c))])' \
  '    bytes_concat_all([out, byte_of(lower_byte(c))])'
arm "a Cell field is derivable" \
  '  else if name == b"Cell" || name == b"Task" { Refused(WHandle) }' \
  '  else if name == b"Task" { Refused(WHandle) }'
arm "a Map key is not required to be ordered" \
  '            let key = if simple == b"Map" && len(c.args) > 0 { check_te(DOrd, at(c.args, 0)) } else { None };' \
  '            let key = None;'

echo
echo "==> the emitter"
arm "a variant is tagged by its position rather than its name" \
  '    bytes_concat_all([pat, b" -> ", e.rt, b"variant(\"", v.name.name, b"\", [", join(values, b", "), b"])"])' \
  '    bytes_concat_all([pat, b" -> ", e.rt, b"variant(\\"", num(len(binders)), b"\\", [", join(values, b", "), b"])"])'
arm "a record decodes its fields in reverse order" \
  '    let i = len(fields) - 1 - k;
    let f = at(fields, i);' \
  '    let i = k;
    let f = at(fields, i);'
arm "a named field type is inlined through the runtime rather than composed by name" \
  '          Nominal -> match c.name.module { Some(m) -> bytes_concat_all([m.name, b"::", call]), None -> call },' \
  '          Nominal -> bytes_concat(e.rt, call),'
arm "the emitter binder ignores a type parameter that starts with d" \
  '    if fold(params, false, |acc: Bool, p: Ident| acc || bytes_starts_with(p.name, x)) { Continue(bytes_concat(x, b"_")) } else { Stop(x) });' \
  '    Stop(x));'
arm "a selective import of the runtime module gets no binder" \
  '                  let b = free_binder(ex, def.deriver);' \
  '                  let b = b"";'

echo
echo "==> the diagnostics"
arm "an orphan derive is silent" \
  '  report(ex, diag1(b"E0208", ex.m, def.target.span, if near { 2 } else { 1 }))' \
  '  ex'
arm "a second derivation of one name is generated again" \
  '        Some(prev) -> collision(ex, def, prev),' \
  '        Some(prev) -> not_derivable(ex, def, { span: def.span, has_note: false, variant: None }),'
arm "a refused field blames the derive line rather than the field" \
  '          Some(b) -> Some({ span: span_to(f.name.span, ty_span(f.ty)), has_note: b.has_note, variant: b.variant }),' \
  '          Some(b) -> Some({ span: b.span, has_note: b.has_note, variant: b.variant }),'

echo
if [ "$fails" -eq 0 ]; then echo "all mutations behaved as declared"; else echo "$fails mutation(s) did not"; exit 1; fi
