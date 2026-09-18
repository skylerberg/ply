#!/usr/bin/env bash
# Arms the differential: each mutation edits a copy of the compiler's modules, `stage`
# bootstraps it, and the agreement suite must report a disagreement.
#
#   ./crates/ply-compiler-diff/tools/arm-harness.sh          # every mutation
#   ./crates/ply-compiler-diff/tools/arm-harness.sh 4 7      # just these
#
# INVALID (the mutant did not run) is not arming: a mutant that cannot compile watches nothing.
# No pipefail: `printf | grep -q` would read printf's SIGPIPE as "no match"; match with [[ ]].
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
src="$root/crates/ply-compiler/ply"

# Each entry: file | sed script | what it corrupts | which property watches it.
# No sed script may contain `|`.
mutations=(
"items.ply|s/constraints: cs, spec: sp,/constraints: [], spec: sp,/|a DROPPED FIELD: a fn's \`where\` constraints are parsed and thrown away|every list emits its length"
"items.ply|s/dump_opt(d.effects, dump_row),//|a DROPPED FIELD in the dumper: a fn's effect row is never emitted|every Option emits its presence"
"patterns.ply|s/node: set_pat_span(inner.node, span_to(o.node, cl.node))/node: inner.node/|a WRONG SPAN: a parenthesised pattern keeps its inner span instead of covering the parens|every node leads with its own span"
"exprs.ply|s/bin_expr(c, a.p, o.bp + 1)/bin_expr(c, a.p, o.bp)/|SWAPPED ASSOCIATIVITY: binary operators become right-associative|the tree shape, at every binary operator"
"exprs.ply|s/{op: b\"add\", bp: 5}/{op: b\"add\", bp: 6}/|SWAPPED PRECEDENCE: \`+\` binds as tightly as \`*\`|the tree shape, where two binding powers meet"
"spine.ply|s/Stop(Ok({p: e.p, node: push(s.out, r.node)}))/Stop(Ok({p: e.p, node: s.out}))/|a DROPPED LIST ELEMENT: every comma list loses its last member|every list emits its length"
"exprs.ply|s/node: Some(t) })/node: None })/|a DROPPED OPTION: a parameter's type annotation is parsed and discarded|every Option emits its presence"
"spine.ply|s/l.code == d.code \&\& ls.start == s.start \&\& ls.end == s.end/l.code == d.code/|a WIDENED DEDUP: two diagnostics with one code at two places become one|the diagnostic list, and its length"
"spine.ply|s/{start: also.start, end: also.end, primary: false}/{start: also.start, end: also.end, primary: true}/|a WRONG PRIMARY FLAG: a secondary label claims to be primary|each label's primary flag, and the primary span derived from it"
"patterns.ply|s/LInt(v) -> bytes_concat(word(b\"int\"), payload(num(v)))/LInt(v) -> bytes_concat(word(b\"int\"), payload(num(0 - v)))/|a WRONG SCALAR: every integer literal is dumped negated|every scalar payload"
"spine.ply|s/node: qualified(first, second) }/node: bare(second) }/|a COLLAPSED QUALIFIER: \`store::place\` loses its module and becomes \`place\`|the Option inside every QName, and that name node's span"
"items.ply|s/item_at(c, a.p, VPub, Some(a.node))/item_at(c, a.p, VPriv, Some(a.node))/|a DROPPED ENUM ARM: \`pub\` is consumed and the item comes out private|every enum arm"
"types.ply|s/node: { eff: ef, mode: m, resource: r,/node: { eff: ef, mode: m, resource: None,/|a DROPPED FIELD inside an effect row: an atom loses its resource label|the row atoms desk.ply's projection compares -- clause 2 of the expander tolerance"
"spine.ply|s/{ p: with_gt_split(p, p.pos), node: {start: s.start, end: s.start + 1} }/{ p: p, node: {start: s.start, end: s.start + 1} }/|a LOST TOKEN REWRITE: \`>=\` closing a type parameter list no longer leaves an \`=\` behind|the \`type Pair<a>= a\` split, and everything after it in the file"
"items.ply|s/rec(d.span, b\"tst\"), rec(d.name_span, b\"tnm\")/rec(d.span, b\"tst\"), rec(d.span, b\"tnm\")/|a WRONG SPAN on a leaf: a test's label span becomes the whole item's|every node leads with its own span"
"exprs.ply|s/dump_opt(v.tail, dump_expr)/dump_opt(None, dump_expr)/|a DROPPED TAIL: a block's tail expression is never emitted|every Option emits its presence"
"exprs.ply|s/named: push(s.named, n)/named: s.named/|a DISCARDED NAMED ARGUMENT: \`name: value\` is lexed, parsed and thrown away|the named-argument list's length"
"exprs.ply|s/span: span_to(expr_span(s.node), q.node)/span: expr_span(s.node)/|a WRONG SPAN on the try operator: the \`?\` byte falls outside its own node|every node leads with its own span"
"exprs.ply|s/Some(b) -> ERecordUpdate({ span: sp, base: b, fields: fields })/Some(b) -> ERecord({ span: sp, fields: fields })/|a COLLAPSED SUGAR NODE: \`{..b, f: e}\` becomes a plain record and the base vanishes|every enum arm, and the node the port must NOT expand"
"exprs.ply|s/if allow { Ok({ p: p, node: Some(e) }) }/if allow { Ok({ p: p, node: None }) }/|a DROPPED OPTION: a parameter's default expression is parsed and discarded|every Option emits its presence"
"exprs.ply|s/Some(f) -> push_diag(s.p, diag2(argument_order(), expr_span(e), f, 1)),/Some(f) -> s.p,/|a MISSING DIAGNOSTIC: a positional argument after a named one is no longer refused|the diagnostic list, and its length"
"lexer.ply|s/TPunct(b\"question\")/TPunct(b\"percent\")/|a WRONG TOKEN: the \`?\` byte lexes as \`%\`|the token vocabulary the parser is built on"
)

want=("${@:-}"); [ $# -eq 0 ] && want=()
armed=0; survived=0; invalid=0
declare -a survivors

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

run_suite() {                    # $1 source dir -> prints the suite's output
  ( cd "$root" \
    && cargo run --offline -q -p ply-compiler-diff --bin stage -- "$1" >/dev/null \
    && PLY_C_EMITTER="ply:$1" \
       cargo test --offline --test suite -- agreement:: --test-threads=2 2>&1 )
}

echo "==> the unmutated tree must be green before any mutation means anything"
base="$work/base"; mkdir -p "$base"
cp "$src"/*.ply "$base/"
out=$(run_suite "$base")
if [[ "$out" == *"test result: ok"* && "$out" != *"disagree on"* ]]; then
  echo "    green"
else
  echo "    NOT GREEN -- arming is meaningless until it is:"; printf '%s\n' "$out" | tail -30
  exit 1
fi
echo

n=0
for entry in "${mutations[@]}"; do
  n=$((n + 1))
  if [ ${#want[@]} -gt 0 ]; then
    hit=0; for w in "${want[@]}"; do [ "$w" = "$n" ] && hit=1; done
    [ "$hit" -eq 1 ] || continue
  fi
  file="${entry%%|*}";  rest="${entry#*|}"
  script="${rest%%|*}"; rest="${rest#*|}"
  what="${rest%%|*}";   watches="${rest#*|}"

  dir="$work/m$n"; rm -rf "$dir"; mkdir -p "$dir"
  cp "$src"/*.ply "$dir/"
  before=$(md5 -q "$dir/$file" 2>/dev/null || md5sum "$dir/$file" | cut -d' ' -f1)
  sed -i '' "$script" "$dir/$file" 2>/dev/null || sed -i "$script" "$dir/$file"
  after=$(md5 -q "$dir/$file" 2>/dev/null || md5sum "$dir/$file" | cut -d' ' -f1)
  printf '%2d. %s\n    (%s; watched by: %s)\n' "$n" "$what" "$file" "$watches"
  if [ "$before" = "$after" ]; then
    echo "    NOT APPLIED -- the sed matched nothing, so this mutation tested itself and not the parser"
    invalid=$((invalid + 1)); echo; continue
  fi

  out=$(run_suite "$dir")
  if [[ "$out" == *"disagree on"* ]]; then
    echo "    ARMED -- $(printf '%s\n' "$out" | grep -c 'disagree on') input(s) disagreed:"
    printf '%s\n' "$out" | grep -m2 -A3 'disagree on' | sed 's/^/      /' | head -8
    armed=$((armed + 1))
  elif [[ "$out" == *"test result: ok"* ]]; then
    echo "    SURVIVED -- the comparison stayed green"
    survived=$((survived + 1)); survivors+=("$n. $what")
  else
    echo "    INVALID -- the mutant did not run, so it watched nothing:"
    printf '%s' "$out" | grep -E "^error|panicked at|failed \(" | head -3 | sed 's/^/      /'
    invalid=$((invalid + 1))
  fi
  echo
done

echo "================================================================"
echo "armed $armed   survived $survived   invalid $invalid"
for s in "${survivors[@]:-}"; do [ -n "$s" ] && echo "  survivor: $s"; done
[ "$survived" -eq 0 ] && [ "$invalid" -eq 0 ]
