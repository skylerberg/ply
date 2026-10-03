#!/usr/bin/env bash
# ci-warm.sh PLY          the `ply` program, built or found once, before a job's tests and corpus
#                         lanes start several processes that would each build it at once. A test of a
#                         scratch project is the cheapest run that emits and compiles a unit; nothing
#                         in it is cached. The emitter's lines say what it read back and was asked.
# ci-warm.sh program PLY  the run's first `ply`, which builds what the committed runnables are
#                         behind: the builder, then the `ply` program with it. Each build's lines say
#                         what its steps took; none means both were committed or staged already.
# ci-warm.sh used MARK    the C cache and the stages cut to what the `ply`s since MARK used: a load
#                         marks what it reads, so the rest is what earlier runs left that this one
#                         did not read, and every job after would restore it for nothing.
# ci-warm.sh pack TAR     the C cache and the stages as one file, for `shared` on another branch
# ci-warm.sh shared NAME  the artifact NAME, which a run of these sources packed, laid over the C
#                         cache and the stages as files this run wrote. A cache reaches only its own
#                         branch and main; an artifact reaches every run. Sets the step's `found`.
#                         Nothing found, or a download that failed, leaves the run to build.
set -euo pipefail
cache=${PLY_C_CACHE:-/tmp/ply-c-cache}
stage=${PLY_C_STAGE:-/tmp/ply-c-stage}

megabytes() { du -sm "$@" 2>/dev/null | awk '{ s += $1 } END { print s + 0 }'; }

# Unpacked aside and copied in only when whole, so a download cut short leaves no half of an object.
shared() {
  local name=$1 run dir
  # A fork's run names its artifacts as it likes, so only this repository's own are taken.
  run=$(gh api "repos/$GITHUB_REPOSITORY/actions/artifacts?name=$name&per_page=10" \
    -q '[.artifacts[] | select(.expired == false and .workflow_run.head_repository_id == .workflow_run.repository_id)]
        | sort_by(.created_at) | last | .workflow_run.id // empty') || return 1
  [ -n "$run" ] || { echo "no run has packed a stage for these sources"; return 1; }
  dir=$(mktemp -d)
  gh run download "$run" -n "$name" -D "$dir" || return 1
  mkdir "$dir/unpacked"
  zstd -d -c "$dir/stage.tar.zst" | tar -xf - -C "$dir/unpacked" || return 1
  mkdir -p "$cache" "$stage"
  # Copied without their times: `used` keeps what is newer than the job's mark.
  if [ -d "$dir/unpacked/cache" ]; then cp -R "$dir/unpacked/cache/." "$cache/" || return 1; fi
  if [ -d "$dir/unpacked/stage" ]; then cp -R "$dir/unpacked/stage/." "$stage/" || return 1; fi
  rm -rf "$dir"
  echo "$name came back from run $run: $(megabytes "$cache" "$stage") MB of C cache and stages"
}

case "${1:-}" in
  used)
    mark=${2:?usage: ci-warm.sh used MARK}
    [ -f "$mark" ] || { echo "nothing is marked at $mark" >&2; exit 2; }
    before=$(megabytes "$cache" "$stage")
    [ -d "$cache" ] && find "$cache" -type f ! -newer "$mark" -delete
    if [ -d "$stage" ]; then
      for dir in "$stage"/*/; do
        [ -d "$dir" ] || continue
        case "$(basename "$dir")" in
          # Files swept one by one: each an answer of its own, or a build's rows under the front end
          # that published them.
          answered | reused | rows) find "$dir" -type f ! -newer "$mark" -delete ;;
          # A stage goes whole: its `.used` stamp is what a load writes.
          *) [ -n "$(find "$dir" -type f -newer "$mark" -print -quit)" ] || rm -rf "$dir" ;;
        esac
      done
    fi
    find "$cache" "$stage" -mindepth 1 -type d -empty -delete 2>/dev/null || true
    echo "the C cache and stages kept $(megabytes "$cache" "$stage") MB of $before MB"
    ;;
  program)
    ply=${2:?usage: ci-warm.sh program PLY}
    err=$(mktemp)
    PLY_C_PHASES=1 "$ply" --version 2> "$err" || { cat "$err" >&2; exit 1; }
    grep '^phases:' "$err" || echo "nothing was built: the builder and the program were committed or staged"
    rm -f "$err"
    ;;
  pack)
    tar=${2:?usage: ci-warm.sh pack TAR}
    dir=$(mktemp -d)
    mkdir -p "$cache" "$stage"
    ln -s "$cache" "$dir/cache"
    ln -s "$stage" "$dir/stage"
    tar -chf - -C "$dir" cache stage | zstd -q -T0 -3 -o "$tar"
    rm -rf "$dir"
    echo "packed $(megabytes "$cache" "$stage") MB of C cache and stages"
    ;;
  shared)
    name=${2:?usage: ci-warm.sh shared NAME}
    if shared "$name"; then
      echo "found=true" >> "${GITHUB_OUTPUT:-/dev/null}"
    else
      echo "no stage came back: this run builds its own"
    fi
    ;;
  *)
    ply=${1:?usage: ci-warm.sh PLY | program PLY | used MARK | pack TAR | shared NAME}
    dir=$(mktemp -d)
    trap 'rm -rf "$dir"' EXIT
    printf 'test "a unit is compiled" {\n  assert(1 + 1 == 2)\n}\n' > "$dir/m.ply"
    PLY_C_PHASES=1 "$ply" test "$dir" > "$dir/out" 2> "$dir/err" || {
      cat "$dir/out" "$dir/err" >&2
      exit 1
    }
    grep '^phases: emitter' "$dir/err" || echo "the emitter was asked nothing: the test compiled nothing" >&2
    ;;
esac
