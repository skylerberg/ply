#!/usr/bin/env bash
# The hash differential, armed: each mutation below is applied to a copy of
# `hash.ply` and the fast half of `harness/tests/hash.rs` (the bundles and the
# standard library) must go red.
#
#   ./spikes/ply-parser/arm-hash.sh
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary; run cargo build -p ply-cli --bin ply"; exit 2; }
work="$(mktemp -d)"
cp "$here"/{lexer,spine,types,patterns,exprs,items,rewrite,resolve,derive,hash}.ply "$work/"
cp "$work/hash.ply" "$work/hash.orig"
restore() { cp "$work/hash.orig" "$work/hash.ply"; }
trap 'rm -rf "$work"' EXIT
fails=0

run_suite() {
  ( cd "$here/harness" \
    && PLY_BIN="$ply" PLY_PARSER_SRC="$work" \
       cargo test --offline --test hash -- --test-threads=2 \
         the_ply_hasher_agrees_with_ply_hash_on_the_bundles \
         the_ply_hasher_agrees_with_ply_hash_on_the_standard_library 2>&1 )
}

# The pattern is quoted literally; the replacement is perl's, so a `\"` in it is written `\\"`.
mutate() {
  restore
  perl -0pi -e "s/\Q$1\E/$2/" "$work/hash.ply" || return 1
  ! cmp -s "$work/hash.orig" "$work/hash.ply"
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
echo "==> the encoding"
arm "a record type keeps its written field order" \
  '      let sorted = sort_bytes(fields.encoded);
      fold(sorted, u32v(fields.z, len(sorted)), |acc: Nz, b: Bytes| emit(acc, b))' \
  '      let sorted = fields.encoded;
      fold(sorted, u32v(fields.z, len(sorted)), |acc: Nz, b: Bytes| emit(acc, b))'
arm "a braced body is not the same computation as its tail" \
  '    EBlock(b) -> if len(b.stmts) == 0 { match b.tail { Some(t) -> expr(z, t), None -> block(z, b.stmts, b.tail) } } else { block(z, b.stmts, b.tail) },' \
  '    EBlock(b) -> block(z, b.stmts, b.tail),'
arm "commuting lets keep their written order" \
  '      if run < 2 { Continue({..s, i: s.i + 1, order: push(s.order, s.i)}) }' \
  '      if true { Continue({..s, i: s.i + 1, order: push(s.order, s.i)}) }'
arm "a row keeps a duplicated atom" \
  '    if i > 0 && at(sorted, i - 1).bytes == x.bytes { mentioned } else { emit(mentioned, x.bytes) }' \
  '    emit(mentioned, x.bytes)'
arm "a local binder is written by name rather than by level" \
  '    Some(level) -> u32v(tag(z, t_local()), level),
    None -> match value_target(z.index, z.module, q) {' \
  '    Some(level) -> strv(tag(z, t_free()), q.name.name),
    None -> match value_target(z.index, z.module, q) {'
arm "an effect reference drops its slot" \
  '    Some(node) -> u32v(node_ref(z, node), match map_get(z.effects, node) { Some(s) -> s, None -> 0 }),' \
  '    Some(node) -> node_ref(z, node),'
arm "a test hashes without its nondet marker" \
  'pub fn test_def(z: Nz, d: TestDef) -> Nz = expr(boolv(tag(z, t_test()), d.is_nondet), d.body)' \
  'pub fn test_def(z: Nz, d: TestDef) -> Nz = expr(tag(z, t_test()), d.body)'

echo
echo "==> the table"
arm "a reference to a hashed definition is written as a self reference" \
  '      None -> match map_get(m.hashes, node) { Some(h) -> emit(tag(m, t_ref_hash()), h), None -> tag(m, t_ref_self()) },' \
  '      None -> tag(m, t_ref_self()),'
arm "a cyclic component takes the first labelling rather than the settled one" \
  '    if same { Stop(re) } else { Continue(re) }' \
  '    Stop(re)'
arm "the closure is direct rather than transitive" \
  '      if rc != ci && rc >= 0 && rc < len(acc) { names_union(cc, at(acc, rc)) } else { cc }' \
  '      if rc != ci && rc >= 0 && rc < len(acc) { map_insert(cc, at(index.nodes, r).name, true) } else { cc }'
arm "the own form is written with hashes rather than names" \
  '            let form: Finished = encode_item(index, node.module, scc, hashes, true, |z: Nz| normalize_node(z, NFn(def)));' \
  '            let form: Finished = encode_item(index, node.module, scc, hashes, false, |z: Nz| normalize_node(z, NFn(def)));'

echo
if [ "$fails" -eq 0 ]; then echo "all mutations behaved as declared"; else echo "$fails mutation(s) did not"; exit 1; fi
