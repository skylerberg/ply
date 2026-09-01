#!/usr/bin/env bash
#
# Is the binary I am about to measure with built from the tree I am looking at?
#
#   .github/binary-is-current.sh                    # target/release/ply
#   .github/binary-is-current.sh target/debug/ply target/release/ply-corpus
#   .github/binary-is-current.sh --self-test        # watch the check go red
#
# Exit 0 the binary is current · 1 it is STALE · 2 the question cannot be
# answered (no binary, no dep-info).
#
# `find crates -name '*.rs' -newer target/release/ply` is the shape this
# replaces, and it is blind in the one place it is most often pointed:
# `crates/ply-std/src/lib.rs` `include_str!`s all eight stdlib modules into the
# binary, so editing one changes what `import std.http` means and moves no
# `.rs` at all. A round-1 workstream published a headline count taken through
# that hole, and the self-hosting spike opens with a measurement nearly lost to the same class.
#
# Three instruments, cheapest first.
#
#   1. rustc's own dep-info -- `<binary>.d`, written beside the binary at every
#      link, listing exactly the files read to produce it. `target/release/ply.d`
#      lists 152 paths across twelve crates: 144 `.rs` and all eight
#      `crates/ply-std/ply/*.ply`. It is per binary, so there is no table here
#      to go stale, and it covers any `include_str!` anyone adds tomorrow.
#
#   2. The bytes, not the clock -- `ply std --show std.<m>` prints the module
#      source compiled into *this* binary. Diffing it against the file answers
#      the real question rather than a proxy for it: it survives `touch`, a
#      checkout that rewrites mtimes, an rsync, clock skew, and the
#      second-granular window recorded in `crates/ply-eval/src/compiled.rs`'s
#      test-module header. It is only available where `std --show` is, which is
#      `ply` and no other binary: `ply-corpus` and `w6-alloc` embed
#      `crates/ply-corpus/ply/{w4,w5}.ply` with no way to read them back out, so
#      for those two this arm is skipped and says so, and their embedded `.ply`
#      rest on dep-info mtimes alone. A `.ply` whose bytes changed while its
#      mtime went backwards is caught for `ply` and not for them.
#
#   3. What dep-info does not carry -- `Cargo.toml`, `Cargo.lock`,
#      `rust-toolchain*` and `.cargo/config.toml` are cargo's inputs, not
#      rustc's, and appear in no `.d` file. Checked here explicitly. A file
#      newer than the binary, in a crate the binary depends on, that the
#      dep-info does not list is reported as SUSPECT: usually a new module no
#      `mod` declares yet, occasionally a new `include_str!` target.
#
# Timestamps are compared to whole seconds and equality counts as stale. That is
# deliberate and conservative: cargo fingerprints on second-granular mtimes, a
# false STALE costs one rebuild, and a false "current" is what this file exists
# to prevent.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
self_test=0
targets=()

while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) self_test=1; shift ;;
    -h|--help) sed -n '2,49p' "${BASH_SOURCE[0]}" | sed 's|^# \{0,1\}||'; exit 0 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) targets+=("$1"); shift ;;
  esac
done

if stat -f '%m' . >/dev/null 2>&1; then stat_flavour=bsd; else stat_flavour=gnu; fi

mtime_of() {
  if [ "$stat_flavour" = bsd ]; then stat -f '%m' -- "$1"; else stat -c '%Y' -- "$1"; fi
}

# NUL-separated paths on stdin; prints those whose mtime is at or after $1's.
at_or_after() {
  local ref
  ref=$(mtime_of "$1")
  if [ "$stat_flavour" = bsd ]; then
    xargs -0 stat -f '%m %N' -- 2>/dev/null
  else
    xargs -0 stat -c '%Y %n' -- 2>/dev/null
  fi | awk -v ref="$ref" '{ s = $1; $1 = ""; sub(/^ /, ""); if (s + 0 >= ref + 0) print }'
}

# Every path right of the first colon, with `\ `-escaped spaces put back.
deps_of() {
  awk '
    { i = index($0, ":"); if (i == 0) next
      rest = substr($0, i + 1)
      gsub(/\\ /, "\001", rest)
      n = split(rest, a, / +/)
      for (j = 1; j <= n; j++) if (a[j] != "") { gsub(/\001/, " ", a[j]); print a[j] } }
  ' "$1"
}

rel() { printf '%s' "${1#"$root"/}"; }

# 1. dep-info: a listed input that is gone, or not older than the binary.
check_depinfo() {                    # $1 binary, $2 dep-info, $3 scratch
  local bin="$1" dep="$2" tmp="$3" bad=0 d
  deps_of "$dep" | sort -u > "$tmp/deps"
  : > "$tmp/present"
  while IFS= read -r d; do
    if [ -e "$d" ]; then printf '%s\0' "$d" >> "$tmp/present"
    else echo "  GONE     $(rel "$d")"; bad=1; fi
  done < "$tmp/deps"
  : > "$tmp/newer"
  if [ -s "$tmp/present" ]; then at_or_after "$bin" < "$tmp/present" > "$tmp/newer" || true; fi
  if [ -s "$tmp/newer" ]; then
    bad=1
    while IFS= read -r line; do echo "  NEWER    $(rel "${line#* }")"; done < "$tmp/newer"
  fi
  return "$bad"
}

# 2. content: the stdlib bytes inside the binary against the bytes on disk. $2
#    is a directory so --self-test can point it at a corrupted copy without
#    touching the worktree.
check_embedded_stdlib() {            # $1 binary, $2 stdlib dir
  local bin="$1" dir="$2" bad=0 f name
  if ! "$bin" std --digest >/dev/null 2>&1; then
    echo "  NOTE     no content check for $(rel "$bin") -- no \`std --show\`; its embedded .ply are on dep-info mtimes alone"
    return 0
  fi
  for f in "$dir"/*.ply; do
    [ -e "$f" ] || continue
    name=$(basename "$f" .ply)
    if ! "$bin" std --show "std.$name" 2>/dev/null | diff -q - "$f" >/dev/null 2>&1; then
      echo "  EMBEDDED std.$name differs from $(rel "$f") -- the binary holds other bytes"
      bad=1
    fi
  done
  return "$bad"
}

# 3. cargo's inputs, which rustc never sees and no `.d` file lists.
check_cargo_inputs() {               # $1 binary, $2 scratch
  local bin="$1" tmp="$2" bad=0 f
  : > "$tmp/cargo-paths"
  for f in "$root"/Cargo.toml "$root"/Cargo.lock "$root"/rust-toolchain \
           "$root"/rust-toolchain.toml "$root"/.cargo/config.toml \
           "$root"/.cargo/config "$root"/crates/*/Cargo.toml; do
    [ -e "$f" ] && printf '%s\0' "$f" >> "$tmp/cargo-paths"
  done
  : > "$tmp/cargo-newer"
  if [ -s "$tmp/cargo-paths" ]; then
    at_or_after "$bin" < "$tmp/cargo-paths" > "$tmp/cargo-newer" || true
  fi
  if [ -s "$tmp/cargo-newer" ]; then
    bad=1
    while IFS= read -r line; do
      echo "  NEWER    $(rel "${line#* }")  (cargo input; in no dep-info)"
    done < "$tmp/cargo-newer"
  fi
  return "$bad"
}

# A `.rs`/`.ply` newer than the binary, inside a crate the binary depends on,
# that the dep-info does not list. Reported, not fatal: it is the one thing
# dep-info structurally cannot know about, and it is usually a new file.
check_unlisted() {                   # $1 binary, $2 scratch
  local bin="$1" tmp="$2" d
  sed "s|^$root/||" "$tmp/deps" | sed -n 's|^\(crates/[^/]*\)/.*|\1|p' | sort -u > "$tmp/crates"
  [ -s "$tmp/crates" ] || return 0
  : > "$tmp/candidates"
  while IFS= read -r d; do
    [ -d "$root/$d" ] && find "$root/$d" \( -name '*.rs' -o -name '*.ply' \) \
      -not -path '*/target/*' -print0 >> "$tmp/candidates"
  done < "$tmp/crates"
  [ -s "$tmp/candidates" ] || return 0
  at_or_after "$bin" < "$tmp/candidates" | sed 's|^[0-9][0-9]* ||' | sort -u > "$tmp/cand-newer" || true
  comm -23 "$tmp/cand-newer" "$tmp/deps" > "$tmp/unlisted" || true
  if [ -s "$tmp/unlisted" ]; then
    while IFS= read -r d; do
      echo "  SUSPECT  $(rel "$d")  (newer, and in no dep-info -- a new file?)"
    done < "$tmp/unlisted"
  fi
  return 0
}

verdict_for() {                      # 0 current, 1 stale, 2 unanswerable
  local bin="$1" dep tmp rc=0
  case "$bin" in /*) ;; *) bin="$root/$bin" ;; esac
  if [ ! -x "$bin" ]; then
    echo "UNKNOWN  $(rel "$bin") -- no such binary; build it before you measure with it"
    return 2
  fi
  dep="${bin}.d"
  if [ ! -f "$dep" ]; then
    echo "UNKNOWN  $(rel "$bin") -- no $(rel "$dep"); rebuild so rustc writes one"
    return 2
  fi
  # A `.d` NEWER than the binary is normal: cargo refreshes every dep-info at
  # the end of the build, seconds after the link. `ply.d` was 31s newer than
  # `ply` on the build this was written against. Only the other direction is
  # odd -- cargo cannot produce it -- so it is noted and the checks still run.
  if [ "$(mtime_of "$dep")" -lt "$(mtime_of "$bin")" ]; then
    echo "  NOTE     $(rel "$dep") is older than the binary; the binary was not written by this cargo build"
  fi
  tmp=$(mktemp -d)
  check_depinfo "$bin" "$dep" "$tmp" || rc=1
  check_embedded_stdlib "$bin" "$root/crates/ply-std/ply" || rc=1
  check_cargo_inputs "$bin" "$tmp" || rc=1
  check_unlisted "$bin" "$tmp"
  if [ "$rc" -eq 0 ]; then
    echo "current  $(rel "$bin")  ($(wc -l < "$tmp/deps" | tr -d ' ') inputs checked)"
  else
    echo "STALE    $(rel "$bin") -- rebuild before you measure with it"
  fi
  rm -rf "$tmp"
  return "$rc"
}

# `--self-test`: watch both instruments go red. Neither arm touches the
# worktree -- the content arm compares the binary against a corrupted *copy* of
# the stdlib, and the mtime arm writes a dep-info of its own under a scratch
# directory.
run_self_test() {
  local bin="$root/target/release/ply" tmp rc=0 out arc
  [ -x "$bin" ] || { echo "self-test needs $(rel "$bin"); build it first" >&2; exit 2; }
  tmp=$(mktemp -d)

  echo "1. the content instrument, against a corrupted copy of the stdlib"
  cp "$root"/crates/ply-std/ply/*.ply "$tmp/"
  printf '\n// self-test\n' >> "$tmp/http.ply"
  if out=$(check_embedded_stdlib "$bin" "$tmp"); then
    echo "   FAILED -- it did not notice bytes that differ"; rc=1
  else
    echo "   red, as it must be:${out#  }"
  fi

  echo "2. the content instrument, against the real stdlib"
  if out=$(check_embedded_stdlib "$bin" "$root/crates/ply-std/ply"); then
    echo "   green, as it must be"
  else
    echo "   this worktree's stdlib is not the one in this binary:"; echo "$out"; rc=1
  fi

  mkdir -p "$tmp/fake"
  cp "$bin" "$tmp/fake/ply"

  echo "3. the dep-info instrument, against an input newer than the binary"
  touch "$tmp/fake/newer.ply"
  printf '%s: %s\n' "$tmp/fake/ply" "$tmp/fake/newer.ply" > "$tmp/fake/ply.d"
  if out=$(check_depinfo "$tmp/fake/ply" "$tmp/fake/ply.d" "$tmp"); then
    echo "   FAILED -- it did not notice an input newer than the binary"; rc=1
  else
    echo "   red, as it must be:${out#  }"
  fi

  echo "4. the dep-info instrument, against an input the tree no longer has"
  printf '%s: %s\n' "$tmp/fake/ply" "$tmp/fake/deleted.ply" > "$tmp/fake/ply.d"
  if out=$(check_depinfo "$tmp/fake/ply" "$tmp/fake/ply.d" "$tmp"); then
    echo "   FAILED -- it did not notice a listed input that is gone"; rc=1
  else
    echo "   red, as it must be:${out#  }"
  fi

  echo "5. the dep-info instrument, against an input older than the binary"
  # A whole second older, not merely earlier: this check calls equality stale.
  touch -t 202001010000 "$tmp/fake/older.ply"
  touch "$tmp/fake/ply"
  printf '%s: %s\n' "$tmp/fake/ply" "$tmp/fake/older.ply" > "$tmp/fake/ply.d"
  if out=$(check_depinfo "$tmp/fake/ply" "$tmp/fake/ply.d" "$tmp"); then
    echo "   green, as it must be"
  else
    echo "   FAILED -- it called an older input stale:"; echo "$out"; rc=1
  fi

  # Arms 1-5 exercise the instruments in isolation. These two exercise the
  # assembly: a red arm has to become a STALE verdict and a nonzero exit, which
  # is a separate thing to get wrong and the thing every caller actually reads.
  mkdir -p "$tmp/whole"
  cp "$bin" "$tmp/whole/ply"

  echo "6. the assembled verdict: a red arm must become STALE and exit 1"
  touch "$tmp/whole/newer.ply"
  printf '%s: %s\n' "$tmp/whole/ply" "$tmp/whole/newer.ply" > "$tmp/whole/ply.d"
  set +e; out=$(verdict_for "$tmp/whole/ply"); arc=$?; set -e
  if [ "$arc" -eq 1 ] && printf '%s\n' "$out" | grep -q '^STALE'; then
    echo "   red, as it must be: exit 1 and a STALE verdict"
  else
    echo "   FAILED -- a red arm did not reach the exit code (exit $arc):"; echo "$out"; rc=1
  fi

  echo "7. the assembled verdict: nothing red must become current and exit 0"
  touch -t 202001010000 "$tmp/whole/older.ply"
  printf '%s: %s\n' "$tmp/whole/ply" "$tmp/whole/older.ply" > "$tmp/whole/ply.d"
  set +e; out=$(verdict_for "$tmp/whole/ply"); arc=$?; set -e
  if [ "$arc" -eq 0 ] && printf '%s\n' "$out" | grep -q '^current'; then
    echo "   green, as it must be"
  else
    echo "   FAILED -- a clean tree did not reach exit 0 (exit $arc):"; echo "$out"; rc=1
  fi

  rm -rf "$tmp"
  [ "$rc" -eq 0 ] && echo "self-test: both instruments, and the verdict they feed, were seen to fail and to pass"
  return "$rc"
}

if [ "$self_test" -eq 1 ]; then
  run_self_test
  exit $?
fi

if [ ${#targets[@]} -eq 0 ]; then targets=("target/release/ply"); fi

status=0
for t in "${targets[@]}"; do
  set +e
  verdict_for "$t"
  rc=$?
  set -e
  if [ "$rc" -gt "$status" ]; then status="$rc"; fi
done
exit "$status"
