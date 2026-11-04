#!/usr/bin/env bash
set -euo pipefail
cache=${PLY_C_CACHE:-/tmp/ply-c-cache}
stage=${PLY_CI_STAGE:-/tmp/ply-c-stage}

megabytes() { du -sm "$@" 2>/dev/null | awk '{ s += $1 } END { print s + 0 }'; }

shared() {
  local name=$1 run
  run=$(gh api "repos/$GITHUB_REPOSITORY/actions/artifacts?name=$name&per_page=10" \
    -q '[.artifacts[] | select(.expired == false and .workflow_run.head_repository_id == .workflow_run.repository_id)]
        | sort_by(.created_at) | last | .workflow_run.id // empty') || return 1
  [ -n "$run" ] || { echo "no run has packed a stage for these sources"; return 1; }
  laid "$run" "$name" cache stage
}

placed() {
  local from=$1 to=$2
  [ -d "$from" ] || return 0
  if [ -d "$to" ] && [ -n "$(ls -A "$to")" ]; then
    cp -R "$from/." "$to/"
  else
    rm -rf "$to"
    mkdir -p "$(dirname "$to")"
    mv "$from" "$to"
  fi
}

laid() {
  local run=$1 name=$2 dir part
  shift 2
  dir=$(mktemp -d)
  gh run download "$run" -n "$name" -D "$dir" || return 1
  mkdir "$dir/unpacked"
  zstd -d -c "$dir/stage.tar.zst" | tar -xf - -C "$dir/unpacked" "$@" || return 1
  rm -f "$dir/stage.tar.zst"
  for part in "$@"; do
    case $part in
      cache) placed "$dir/unpacked/cache" "$cache" || return 1 ;;
      stage) placed "$dir/unpacked/stage" "$stage" || return 1 ;;
    esac
  done
  rm -rf "$dir"
  echo "$name came back from run $run: $(megabytes "$cache" "$stage") MB of $*"
}

case "${1:-}" in
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
    if laid "${GITHUB_RUN_ID:?the run whose artifact this is}" "$name" cache; then
      echo "found=true" >> "${GITHUB_OUTPUT:-/dev/null}"
    else
      echo "could not download the stage $name: this job starts from the cache"
    fi
    ;;
  *)
    echo "usage: ci-warm.sh pack TAR | shared NAME | this-run NAME" >&2
    exit 2
    ;;
esac
