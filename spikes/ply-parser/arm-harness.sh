#!/usr/bin/env bash
#
# Arms the differential in `harness/`.
#
#   ./spikes/ply-parser/arm-harness.sh          # every mutation (22 of them)
#   ./spikes/ply-parser/arm-harness.sh 4 7      # just these
#
# House rule 6: "the signature defect here is a green result over unexplored
# space. A comparison that cannot go red proves nothing." So every claim the
# harness makes is checked by breaking the thing it watches and confirming the
# comparison notices.
#
# Each mutation edits a copy of the six modules under a temp directory --
# `PLY_PARSER_SRC` points the harness at it -- and the worktree is never
# touched. Three outcomes, and the script distinguishes them because two of them
# look alike from the outside:
#
#   ARMED     the comparison reported a disagreement. What we want.
#   INVALID   the mutant does not typecheck or does not run, so the suite failed
#             for a reason that is not a disagreement. This is *not* arming: a
#             mutation that cannot compile watches nothing, and counting it
#             would be the same error as counting a skipped test as a pass.
#   SURVIVED  the mutant ran and the comparison stayed green. A hole, and the
#             script says which one.
#
# The mutations are chosen to be the three classes the brief names -- a dropped
# field, a wrong span, a swapped associativity -- plus one per structural
# property the dump grammar claims to have, plus one per feature the port
# learned on 2026-08-30.
#
# **Two changes to the table on 2026-08-30, both recorded rather than done
# quietly.**
#
#   * #7 was `types.ply|s/node: { name: n, ty: Some(t),/node: { name: n, ty:
#     None,/`, and its anchor no longer exists: `param` moved to `exprs.ply`
#     when a parameter gained a default expression (`GAPS.md` 11R.D). It is
#     REPLACED, not dropped, by the same corruption at the same parser in its
#     new home -- a parameter's type annotation parsed and discarded -- so the
#     property it watches is unchanged.
#   * #17 through #22 are new, one per dump edge the port gained: the named
#     argument list, `ETry`'s span, `ERecordUpdate`'s base, `Param`'s default,
#     `E0124`, and the `?` byte itself. #17 is the one worth naming: it is
#     exactly the port `GAPS.md` 11R.N showed would have PASSED before this
#     change -- one that reads `name`, `:`, `value` and throws all three away.
#
# #13 corrupts a row's atoms. Its old description said it stood behind clause 2
# of `only_the_expanders_diagnostics`; that tolerance is gone with the
# projection, and the mutation now watches `desk.ply`'s rows directly, which is
# a stronger thing for it to watch.
# No `pipefail`. `printf '%s' "$big" | grep -q x` sets the pipeline's status to
# printf's SIGPIPE under it, because grep exits at the first match and closes the
# pipe -- so a mutation with a lot of output reads as "no match". The first run
# of this script scored three ARMED mutations as INVALID for exactly that, and
# the tell was that they were the three with the most disagreements. Matching is
# done with `[[ == * ]]` below, which has no pipeline at all.
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || { echo "no ply binary at $ply; cargo build -p ply-cli --bin ply --release" >&2; exit 2; }

# Each entry: file | sed script | what it corrupts | which property watches it.
#
# `|` separates the fields, so no sed script below may contain one; the two that
# want an alternation use a second `-e` instead.
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
"exprs.ply|s/named: push(s.named, n)/named: s.named/|a DISCARDED NAMED ARGUMENT: \`name: value\` is lexed, parsed and thrown away|the named-argument list's length, which nothing emitted before 2026-08-30 (GAPS.md 11R.N)"
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
  ( cd "$here/harness" \
    && PLY_BIN="$ply" PLY_PARSER_SRC="$1" \
       cargo test --offline --test agreement -- --test-threads=2 2>&1 )
}

echo "==> the unmutated tree must be green before any mutation means anything"
base="$work/base"; mkdir -p "$base"
cp "$here"/{lexer,spine,types,patterns,exprs,items}.ply "$base/"
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
  cp "$here"/{lexer,spine,types,patterns,exprs,items}.ply "$dir/"
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
