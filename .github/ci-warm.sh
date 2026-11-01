#!/usr/bin/env bash
set -euo pipefail
cache=${PLY_C_CACHE:-/tmp/ply-c-cache}
stage=${PLY_C_STAGE:-/tmp/ply-c-stage}
stores=$cache/bodies

megabytes() { du -sm "$@" 2>/dev/null | awk '{ s += $1 } END { print s + 0 }'; }

shared() {
  local name=$1 run
  run=$(gh api "repos/$GITHUB_REPOSITORY/actions/artifacts?name=$name&per_page=10" \
    -q '[.artifacts[] | select(.expired == false and .workflow_run.head_repository_id == .workflow_run.repository_id)]
        | sort_by(.created_at) | last | .workflow_run.id // empty') || return 1
  [ -n "$run" ] || { echo "no run has packed a stage for these sources"; return 1; }
  laid "$run" "$name"
}

laid() {
  local run=$1 name=$2 dir
  dir=$(mktemp -d)
  gh run download "$run" -n "$name" -D "$dir" || return 1
  mkdir "$dir/unpacked"
  zstd -d -c "$dir/stage.tar.zst" | tar -xf - -C "$dir/unpacked" || return 1
  mkdir -p "$cache" "$stage"
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
    for dir in "$stores"/*/; do
      [ -d "$dir" ] || continue
      [ "$dir.used" -nt "$mark" ] || [ "$dir" -nt "$mark" ] || rm -rf "$dir"
    done
    [ -d "$cache" ] && find "$cache" -path "$stores/*" -type d -prune -o -type f ! -newer "$mark" -print0 |
      xargs -0 rm -f --
    if [ -d "$stage" ]; then
      for dir in "$stage"/*/; do
        [ -d "$dir" ] || continue
        case "$(basename "$dir")" in
          answered | reused) find "$dir" -type f ! -newer "$mark" -delete ;;
          *) [ -n "$(find "$dir" -newer "$mark" -print -quit)" ] || rm -rf "$dir" ;;
        esac
      done
    fi
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
  this-run)
    name=${2:?usage: ci-warm.sh this-run NAME}
    if laid "${GITHUB_RUN_ID:?the run whose artifact this is}" "$name"; then
      echo "found=true" >> "${GITHUB_OUTPUT:-/dev/null}"
    else
      echo "could not download the stage $name: this job starts from the cache"
    fi
    ;;
  foreign)
    [ -d "$stage" ] && find "$stage" -mindepth 1 -maxdepth 1 ! -name memo-std -exec rm -rf {} +
    echo "kept $(megabytes "$cache") MB of the C cache and $(megabytes "$stage") MB of the std records from another toolchain, and deleted its stages"
    ;;
  *)
    echo "usage: ci-warm.sh used MARK | pack TAR | shared NAME | this-run NAME | foreign" >&2
    exit 2
    ;;
esac
