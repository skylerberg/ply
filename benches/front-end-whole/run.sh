#!/usr/bin/env bash
# The whole front end, phase by phase: PRE-REGISTERED.md's protocol, and nothing else.
#
#   ./benches/front-end-whole/run.sh                 # writes benches/front-end-whole/raw.txt
#   ./benches/front-end-whole/run.sh --dir <probe>   # an already-built probe project
#
# Refuses to start above the load gate (exit 3) and refuses a stale binary (exit 2).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
spike="$root/spikes/ply-parser"
raw="$here/raw.txt"

load1() { uptime | sed 's/.*load averages*: *//' | awk -F'[ ,]+' '{print $1}'; }

echo "==> instrument check: is the binary the one this tree would produce?"
"$root/.github/binary-is-current.sh" || { echo "STALE -- rebuild before measuring" >&2; exit 2; }
l=$(load1)
echo "==> load before: $(uptime)"
awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' || { echo "load $l is above the gate of 4; not measuring" >&2; exit 3; }

dir=""
if [ "${1:-}" = "--dir" ]; then dir="$2"; shift 2; fi
if [ -z "$dir" ]; then
  dir="$(mktemp -d)"
  cp "$spike"/{lexer,spine,types,patterns,exprs,items,rewrite,resolve,derive,tycore,infer,hash}.ply "$dir/"
  python3 - "$root/crates/ply-std/ply" "$root/examples/desk.ply" "$dir" <<'PY'
import sys, pathlib
std, example, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])

def literal(src: bytes) -> str:
    o = []
    for ch in src:
        if ch == 0x22: o.append('\\"')
        elif ch == 0x5c: o.append('\\\\')
        elif 0x20 <= ch < 0x7f: o.append(chr(ch))
        else: o.append('\\x%02x' % ch)
    return 'b"%s"' % ''.join(o)

lines = [
    "import items (parse)",
    "import rewrite (expand)",
    "import resolve (Source, Mod, Resolved_, Failed, resolve_tables, resolve_modules)",
    "import derive (expand_derives)",
    "import infer (check_program)",
    "import hash (build_index, hash_index)",
    "",
]
names = []
for path in sorted(std.glob("*.ply")):
    name = "std_" + path.stem
    names.append(name)
    lines.append('fn src_%s() -> Bytes = %s' % (name, literal(path.read_bytes())))
    lines.append('fn name_%s() -> Bytes = b"std.%s"' % (name, path.stem))
lines.append('fn src_example() -> Bytes = %s' % literal(example.read_bytes()))
lines.append('fn name_example() -> Bytes = b"%s"' % example.stem)
names.append("example")
lines.append("")
lines.append("fn sources() -> List<Source> = [%s]" % ", ".join(
    "{ name: name_%s(), src: src_%s() }" % (n, n) for n in names))
lines.append("""
// Every phase re-runs the ones before it, so a phase is the difference between two rows.
fn parsed() -> List<Mod> =
  map(range(0, len(sources())), |i: Int| {
    let s = list_at(sources(), i);
    match s {
      Some(src) -> {
        let r = parse(src.src);
        let x = expand(r.node, r.p.uses_sets, r.p.diags);
        { name: src.name, module: x.module, derived: [] }
      },
      None -> panic("an index sources() holds"),
    }
  })

fn expanded() -> List<Mod> = {
  let ps = parsed();
  map(range(0, len(ps)), |i: Int|
    match list_at(ps, i) {
      Some(m) -> { let d = expand_derives(i, m.module); { name: m.name, module: d.module, derived: d.derived } },
      None -> panic("an index parsed() holds"),
    })
}

test "row: parse" { assert(len(parsed()) > 0) }

test "row: expand" { assert(len(expanded()) > 0) }

test "row: tables" {
  match resolve_tables(expanded()) { Resolved_(x) -> assert(len(x.mods) > 0), Failed(_) -> assert(false) }
}

test "row: resolve" {
  match resolve_modules(expanded()) { Resolved_(x) -> assert(len(x.mods) > 0), Failed(_) -> assert(false) }
}

test "row: check" {
  match resolve_modules(expanded()) { Resolved_(x) -> assert(check_program(x.r, x.mods).ok), Failed(_) -> assert(false) }
}

test "row: hash" {
  match resolve_modules(expanded()) {
    Resolved_(x) -> { let built = build_index(x.mods, x.r); assert(len(hash_index(built.index).defs) > 0) },
    Failed(_) -> assert(false),
  }
}
""")
(out / "probe.ply").write_text("\n".join(lines))
PY
fi
echo "==> probe project: $dir"

: > "$raw"
echo "binary $(shasum -a 256 "$ply" | cut -c1-16)" >> "$raw"
echo "load-before $l" >> "$raw"

workers=4
# Warm the front-end cache once; every arm then reads the same warm cache.
"$ply" test "$dir" --no-cache --jobs $workers --filter "row:nothing-matches" >/dev/null 2>&1 || true

run_arm() {                       # $1 label, $2... command; prints "user wall"
  local out u w
  out=$( { /usr/bin/time -p "${@:2}" >/dev/null; } 2>&1 )
  u=$(printf '%s\n' "$out" | awk '/^user/{print $2}')
  w=$(printf '%s\n' "$out" | awk '/^real/{print $2}')
  [ -n "$u" ] && [ -n "$w" ] || { echo "the timer reported nothing for $1: $out" >&2; exit 1; }
  echo "$u $w"
}

arm_cmd() {                       # $1 arm -> the command, as words on stdout
  case "$1" in
    none|null) echo "$ply test $dir --no-cache --jobs $workers --filter row:" ;;
    wide)      echo "$ply test $dir --no-cache --jobs $workers --filter row: --backend cranelift" ;;
    floor)     echo "$ply test $dir --no-cache --jobs $workers --filter row:nothing-matches" ;;
  esac
}

arms=(none null wide)
n=3
echo "blocks $n" >> "$raw"
for ((b=1; b<=n; b++)); do
  for ((k=0; k<3; k++)); do
    arm=${arms[$(( (b-1+k) % 3 ))]}
    r=$(run_arm "$arm" $(arm_cmd "$arm"))
    echo "block $b $arm $r" | tee -a "$raw"
  done
done
r=$(run_arm floor $(arm_cmd floor)); echo "floor $r" | tee -a "$raw"

# The phase breakdown and the seam counts, once per arm, from the report a user reads.
for arm in none wide; do
  json=$(eval "$(arm_cmd "$arm") --json" 2>/dev/null || true)
  printf 'phases %s %s\n' "$arm" "$(printf '%s' "$json" | python3 -c 'import json,sys
r=json.load(sys.stdin)
tests=r.get("tests") or r.get("results") or []
out=[]
for t in tests:
    label=t.get("name") or t.get("label") or ""
    ms=t.get("ms") or t.get("duration_ms") or t.get("elapsed_ms")
    if label.startswith("row: "): out.append("%s=%s" % (label[5:], ms))
print(" ".join(out))' 2>/dev/null || echo unread)" | tee -a "$raw"
  if [ "$arm" = wide ]; then
    printf 'counts %s %s\n' "$arm" "$(printf '%s' "$json" | python3 -c 'import json,sys
r=json.load(sys.stdin); b=r.get("backend") or {}
print(" ".join(f"{k}={b.get(k)}" for k in ("fragment","offered","entered","declined")))' 2>/dev/null || echo unread)" | tee -a "$raw"
  fi
done

echo "load-after $(load1)" >> "$raw"
echo "==> load after: $(uptime)"
"$root/.github/binary-is-current.sh" >/dev/null || { echo "the binary went STALE during the series; void" >&2; exit 2; }
python3 "$here/analyze.py" "$raw"
