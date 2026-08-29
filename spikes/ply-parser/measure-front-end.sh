#!/usr/bin/env bash
#
# F1-F6 of /tmp/ply-parser-spike/PREREGISTRATION-MULTIPLIER.md: the Ply lexer,
# the Ply parser and the Rust front end, all in one sitting at one load.
#
#   ./spikes/ply-parser/measure-front-end.sh            # the five registered files
#   ./spikes/ply-parser/measure-front-end.sh <file>...  # others
#
# `measure-multiplier.sh` took (P-Z)/(L-Z) and nothing else. This adds the two
# things that file's own §H5 caveat says are missing: the *dump* term, so that
# ADR 0020 §6.1's ~17,000 tokens/s can be re-taken in the probe shape it was
# originally taken in rather than a different one, and the Rust front end, so
# that ADR 0020 §8's "not a clean single-sitting figure" stops being true.
#
# Five probes per file, five project directories each holding the same six
# modules so module typechecking is identical and cancels in every difference:
#
#   Z   bytes_len(source())                     start, typecheck, the literal
#   L   len(lexer::lex(source()).toks)          and the lexer
#   P   len(items::parse(source()).node.items)  and the parser
#   LD  string_len(lexer::dump(source()))       lexer + its dump  (ADR 0020 §6.1's shape)
#   PD  string_len(items::dump(source()))       parser + the tree dump
#
# Minimum user CPU over N runs, N=5 where one run is under 2 s and N=3
# otherwise -- decided from run 1, which is kept and counted. Every run printed,
# `uptime` before and after, nothing discarded. The binary is checked before the
# series and again after it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
plydump="$root/spikes/ply-lexer/harness/target/release/plydump"

echo "==> instrument check (house rule 5), before"
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
    "Z":  "fn main() -> Int = bytes_len(source())",
    "L":  "fn main() -> Int = len(lex(source()).toks)",
    "P":  "fn main() -> Int = len(parse(source()).node.items)",
    "LD": "fn main() -> Int = string_len(dump(source()))",
    "PD": "fn main() -> Int = string_len(dump(source()))",
}[kind]
imports = {"Z": "", "L": "import lexer (lex)\n", "P": "import items (parse)\n",
           "LD": "import lexer (dump)\n", "PD": "import items (dump)\n"}[kind]
open(out + "/probe.ply", "w").write(
    "%sfn source() -> Bytes = %s\n%s\n" % (imports, lit, body))
PY
}

# Every run printed; answers "<min-user> <min-wall> <spread%> <n>".
series() {                       # $1 dir, $2 label
  local best_u="" worst_u="" best_w="" i out u w n=5 ok
  for i in 1 2 3 4 5; do
    [ "$i" -gt "$n" ] && break
    rm -rf "$1/.ply-cache"
    out=$( { /usr/bin/time -p "$ply" run "$1" --json; } 2>&1 )
    ok=$(printf '%s\n' "$out" | grep -c '"ok": *true' || true)
    w=$(printf '%s\n' "$out" | awk '/^real/{print $2}')
    u=$(printf '%s\n' "$out" | awk '/^user/{print $2}')
    if [ "$ok" -eq 0 ]; then
      printf '      %s run %d: REFUSED -- the program did not report ok:true\n' "$2" "$i" >&2
      printf '%s\n' "$out" | head -5 >&2
      echo "UNRAN UNRAN UNRAN 0"; return 0
    fi
    printf '      %s run %d: user %ss  wall %ss\n' "$2" "$i" "$u" "$w" >&2
    # N is decided from run 1 and then held: 5 under two seconds, 3 otherwise.
    if [ "$i" -eq 1 ] && awk "BEGIN{exit !($w >= 2.0)}"; then n=3; fi
    if [ -z "$best_u" ] || awk "BEGIN{exit !($u < $best_u)}"; then best_u="$u"; best_w="$w"; fi
    if [ -z "$worst_u" ] || awk "BEGIN{exit !($u > $worst_u)}"; then worst_u="$u"; fi
  done
  awk -v b="$best_u" -v W="$worst_u" -v w="$best_w" -v n="$n" \
    'BEGIN{ s = (b > 0) ? 100*(W-b)/b : 0; printf "%s %s %.0f %d\n", b, w, s, n }'
}

echo "### Part 1 -- the Ply lexer and the Ply parser"
echo
for f in "${files[@]}"; do
  name=$(basename "$f")
  bytes=$(wc -c < "$f" | tr -d ' ')
  tokens=$("$plydump" "$f" | awk -F: '$3!="!"' | wc -l | tr -d ' ')
  echo "  --- $name ($bytes bytes, $tokens tokens) ---" >&2
  for k in Z L P LD PD; do write_probe "$f" "$k" "$work/$name.$k"; done
  read -r zu zw zs zn <<<"$(series "$work/$name.Z"  Z)"
  read -r lu lw ls ln <<<"$(series "$work/$name.L"  L)"
  read -r pu pw ps pn <<<"$(series "$work/$name.P"  P)"
  read -r du dw ds dn <<<"$(series "$work/$name.LD" LD)"
  read -r qu qw qs qn <<<"$(series "$work/$name.PD" PD)"
  echo "RAW $name $bytes $tokens Z=$zu/$zs% L=$lu/$ls% P=$pu/$ps% LD=$du/$ds% PD=$qu/$qs% n=$zn,$ln,$pn,$dn,$qn"
  echo "WALL $name Z=$zw L=$lw P=$pw LD=$dw PD=$qw"
done

echo
echo "### Part 2 -- the Rust front end, same sitting"
echo
ex="$root/examples"
extok=$("$plydump" "$ex"/agreement.ply | awk -F: '$3!="!"' | wc -l)   # warm the FS
extok=0
for f in "$ex"/*.ply; do
  extok=$(( extok + $("$plydump" "$f" | awk -F: '$3!="!"' | wc -l | tr -d ' ') ))
done
exbytes=$(cat "$ex"/*.ply | wc -c | tr -d ' ')
echo "  examples/: $(ls "$ex"/*.ply | wc -l | tr -d ' ') files, $exbytes bytes, $extok tokens"

check_series() {                 # $1 cold|warm, $2 n
  local best_u="" best_w="" i out u w
  for i in $(seq 1 "$2"); do
    [ "$1" = cold ] && rm -rf "$ex/.ply-cache"
    out=$( { /usr/bin/time -p "$ply" check "$ex" >/dev/null; } 2>&1 )
    w=$(printf '%s\n' "$out" | awk '/^real/{print $2}')
    u=$(printf '%s\n' "$out" | awk '/^user/{print $2}')
    printf '      check %s run %d: user %ss  wall %ss\n' "$1" "$i" "$u" "$w" >&2
    if [ -z "$best_u" ] || awk "BEGIN{exit !($u < $best_u)}"; then best_u="$u"; best_w="$w"; fi
  done
  echo "$best_u $best_w"
}
"$ply" check "$ex" >/dev/null            # populate the cache for the warm series
read -r wu ww <<<"$(check_series warm 5)"
read -r cu cw <<<"$(check_series cold 5)"
echo "RUST cold user=$cu wall=$cw   warm user=$wu wall=$ww   tokens=$extok bytes=$exbytes"

echo
echo "==> load after: $(uptime)"
echo "==> instrument check, after"
"$root/.github/binary-is-current.sh"
