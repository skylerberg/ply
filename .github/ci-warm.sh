#!/usr/bin/env bash
# ci-warm.sh used MARK    the C cache and the stages cut to what the `ply`s since MARK used: a load
#                         marks what it reads, so the rest is what earlier runs left that this one
#                         did not read, and every job after would restore it for nothing. A stage
#                         and a store of kept records go whole, by their `.used`.
# ci-warm.sh pack TAR     the C cache and the stages as one file, for `handed` in this run's other
#                         jobs and `shared` on another branch
# ci-warm.sh shared NAME  the artifact NAME, which a run of these sources packed, laid over the C
#                         cache and the stages as files this run wrote. A cache reaches only its own
#                         branch and main; an artifact reaches every run. Sets the step's `found`.
#                         Nothing found, or a download that failed, leaves the run to build.
# ci-warm.sh handed NAME  likewise the artifact NAME of this run, which `build-ply` packed for the
#                         run's other jobs
# ci-warm.sh foreign      the C cache and stages another toolchain's run left, less the stages: each
#                         file of the C cache is named by what decides it, so it serves any toolchain
#                         that names it, but a stage's records do not say which compiler wrote them
#                         (every builder writes `memo-std`)
set -euo pipefail
cache=${PLY_C_CACHE:-/tmp/ply-c-cache}
stage=${PLY_C_STAGE:-/tmp/ply-c-stage}
# The stores of kept records, each a directory `sweep` takes whole (its `STORES`).
stores=$cache/bodies

megabytes() { du -sm "$@" 2>/dev/null | awk '{ s += $1 } END { print s + 0 }'; }

shared() {
  local name=$1 run
  # A fork's run names its artifacts as it likes, so only this repository's own are taken.
  run=$(gh api "repos/$GITHUB_REPOSITORY/actions/artifacts?name=$name&per_page=10" \
    -q '[.artifacts[] | select(.expired == false and .workflow_run.head_repository_id == .workflow_run.repository_id)]
        | sort_by(.created_at) | last | .workflow_run.id // empty') || return 1
  [ -n "$run" ] || { echo "no run has packed a stage for these sources"; return 1; }
  laid "$run" "$name"
}

# The artifact NAME of run RUN, unpacked aside and copied in only when whole, so a download cut short
# leaves no half of an object.
laid() {
  local run=$1 name=$2 dir
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
    # A store goes whole, kept where a run since the mark used it: its segments were written by
    # earlier runs, and only its `.used` says this one read them.
    for dir in "$stores"/*/; do
      [ -d "$dir" ] || continue
      [ "$dir.used" -nt "$mark" ] || rm -rf "$dir"
    done
    [ -d "$cache" ] && find "$cache" -path "$stores/*" -type d -prune -o -type f ! -newer "$mark" -print0 |
      xargs -0 rm -f --
    if [ -d "$stage" ]; then
      for dir in "$stage"/*/; do
        [ -d "$dir" ] || continue
        case "$(basename "$dir")" in
          # Files swept one by one, as `sweep`'s `BY_FILE` takes them: each a runnable or an
          # answer of its own.
          answered | reused) find "$dir" -type f ! -newer "$mark" -delete ;;
          # A stage, and the memo stores, go whole: its `.used` stamp is what a load writes.
          *) [ -n "$(find "$dir" -type f -newer "$mark" -print -quit)" ] || rm -rf "$dir" ;;
        esac
      done
    fi
    # A file beside the stages is what an older layout kept, which nothing this run read.
    [ -d "$stage" ] && find "$stage" -maxdepth 1 -type f ! -name '.*' ! -newer "$mark" -delete
    find "$cache" "$stage" -mindepth 1 -type d -empty -delete 2>/dev/null || true
    echo "the C cache and stages kept $(megabytes "$cache" "$stage") MB of $before MB"
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
  handed)
    name=${2:?usage: ci-warm.sh handed NAME}
    if laid "${GITHUB_RUN_ID:?the run whose artifact this is}" "$name"; then
      echo "found=true" >> "${GITHUB_OUTPUT:-/dev/null}"
    else
      echo "the stage $name did not come back: this job starts from the cache"
    fi
    ;;
  foreign)
    rm -rf "$stage"
    echo "kept $(megabytes "$cache") MB of another toolchain's C cache, and none of its stages"
    ;;
  *)
    echo "usage: ci-warm.sh used MARK | pack TAR | shared NAME | handed NAME | foreign" >&2
    exit 2
    ;;
esac
