#!/usr/bin/env bash
#
# M1 and M2 of /tmp/ply-parser-spike/PREREGISTRATION-INTEGRATION.md: the
# lexer-to-parser cost multiplier that the self-hosting spike assumes at 5-10x and that
# only writing the parser could settle.
#
#   ./spikes/ply-parser/measure-multiplier.sh            # the five registered files
#   ./spikes/ply-parser/measure-multiplier.sh <file>...  # others
#
# Three probes per file, each a whole Ply program whose only difference is what
# it does with the source:
#
#   Z  bytes_len(source())                  process start, typecheck, the literal
#   L  len(lexer::lex(source()).toks)       and the lexer
#   P  len(items::parse(source()).node.items) and the parser on top
#
# The multiplier is (P-Z)/(L-Z). Minimum user CPU over N runs, N=5 under two
# seconds and N=3 otherwise; every run printed; `uptime` before and after; no
# run discarded. The binary is checked before the series, not after, because a
# rebuild half way through would invalidate what came before it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"

echo "==> instrument check (house rule 5)"
"$root/.github/binary-is-current.sh" || { echo "STALE -- rebuild before measuring"; exit 1; }
echo "==> load before: $(uptime)"
echo

files=("$@")
if [ ${#files[@]} -eq 0 ]; then
  files=("$root/examples/desk.ply" "$root/crates/ply-std/ply/db.ply"
         "$root/crates/ply-std/ply/http.ply" "$root/crates/ply-std/ply/json.ply"
         "$root/crates/ply-std/ply/router.ply")
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# `python3` writes the probe because the source has to become a `b"..."` literal
# and there is no file-reading host handler: a Ply program holds its input as a
# literal or never sees it.
write_probe() {                  # $1 file, $2 probe kind, $3 out dir
  mkdir -p "$3"
  cp "$here"/{lexer,spine,types,patterns,exprs,items}.ply "$3/"
  python3 - "$1" "$2" "$3" <<'PY'
import sys
src = open(sys.argv[1], "rb").read()
kind, out = sys.argv[2], sys.argv[3]
o = []
for ch in src:
    if ch == 0x22: o.append('\\"')
    elif ch == 0x5c: o.append('\\\\')
    elif 0x20 <= ch < 0x7f: o.append(chr(ch))
    else: o.append('\\x%02x' % ch)
lit = 'b"' + ''.join(o) + '"'
body = {
    "Z": "fn main() -> Int = bytes_len(source())",
    "L": "fn main() -> Int = len(lex(source()).toks)",
    "P": "fn main() -> Int = len(parse(source()).node.items)",
}[kind]
imports = {"Z": "", "L": "import lexer (lex)\n", "P": "import items (parse)\n"}[kind]
open(out + "/probe.ply", "w").write(
    "%sfn source() -> Bytes = %s\n%s\n" % (imports, lit, body))
PY
}

# min user CPU over $2 runs, printing each; answers "<min-user> <min-wall>".
series() {                       # $1 dir, $2 n, $3 label
  local best_u="" best_w="" i out u w
  for i in $(seq 1 "$2"); do
    rm -rf "$1/.ply-cache"
    out=$( { /usr/bin/time -p "$ply" run "$1" --json >/dev/null; } 2>&1 )
    w=$(printf '%s\n' "$out" | awk '/^real/{print $2}')
    u=$(printf '%s\n' "$out" | awk '/^user/{print $2}')
    printf '      %s run %d: user %ss  wall %ss\n' "$3" "$i" "$u" "$w" >&2
    if [ -z "$best_u" ] || awk "BEGIN{exit !($u < $best_u)}"; then best_u="$u"; best_w="$w"; fi
  done
  echo "$best_u $best_w"
}

printf '%-14s %8s %8s %9s %9s %9s %9s %9s %10s\n' \
  file bytes tokens "Z user" "L user" "P user" "L-Z" "P-Z" "(P-Z)/(L-Z)"

for f in "${files[@]}"; do
  name=$(basename "$f")
  bytes=$(wc -c < "$f" | tr -d ' ')
  d="$work/$name"
  for k in Z L P; do write_probe "$f" "$k" "$d.$k"; done
  # Token count, from the probe itself rather than from a second lexer.
  tokens=$("$ply" run "$d.L" --json 2>/dev/null | awk -F': ' '/"value"/{gsub(/[",]/,"",$2); print $2}')

  echo "  --- $name ($bytes bytes, $tokens tokens) ---" >&2
  n=3
  read -r zu zw <<<"$(series "$d.Z" 5 Z)"
  read -r lu lw <<<"$(series "$d.L" $n L)"
  read -r pu pw <<<"$(series "$d.P" $n P)"
  awk -v n="$name" -v b="$bytes" -v t="$tokens" -v z="$zu" -v l="$lu" -v p="$pu" \
      -v lw="$lw" -v pw="$pw" 'BEGIN{
        dl = l - z; dp = p - z;
        r = (dl > 0) ? sprintf("%.2f", dp/dl) : "UNMEASURED";
        printf "%-14s %8d %8s %9s %9s %9s %9.2f %9.2f %10s\n", n, b, t, z, l, p, dl, dp, r;
        if (dp > 0) printf "               lex+parse: %.0f tokens/s, %.0f bytes/s (user CPU)\n", t/dp, b/dp;
      }'
done

echo
echo "==> load after: $(uptime)"
