#!/usr/bin/env bash
# Usage: ci-warm.sh used MARK | pack TAR | shared NAME | this-run NAME | foreign. The C cache and the
# stages are the directories that hold the C compiler's outputs and the `ply` programs' cached
# results.
#
# `used MARK` deletes every file from the C cache and the stages that no `ply` read or wrote since
# the file MARK was created. Earlier runs left those files, and every later job would restore them
# without using them. A stage, or a store of cached records, is deleted or kept as a whole,
# according to its `.used`.
#
# `pack TAR` writes the C cache and the stages into the single file TAR. The same run's other jobs
# download it with `this-run`, and runs on other branches download it with `shared`.
#
# `shared NAME` downloads the artifact NAME, which an earlier run of the same sources packed with
# `pack`, and copies its files into the C cache and the stages. A cache is readable only from its
# own branch and main, but an artifact is readable from every run. It sets the step output `found`.
# If no artifact exists or the download fails, the run builds what it needs.
#
# `this-run NAME` does the same for the artifact NAME that this run's `build-ply` job packed for the
# run's other jobs.
#
# `foreign` deletes the files that another toolchain's run left in the C cache and the stages,
# except the stage `memo-std`. Each file of the C cache is named by the inputs that determine its
# content, and each record in `memo-std` is keyed by its query's code and checked against what it
# read, so any toolchain can use them. Any other stage contains what one toolchain's binary built
# for itself, so other toolchains cannot use it.
set -euo pipefail
cache=${PLY_C_CACHE:-/tmp/ply-c-cache}
stage=${PLY_C_STAGE:-/tmp/ply-c-stage}
# The stores of cached records. `sweep` (its `STORES`) deletes or keeps each store directory as a
# whole.
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

# Downloads the artifact NAME of run RUN and unpacks it in a temporary directory. It copies the
# files into the cache and the stages only after unpacking succeeds, so an interrupted download
# never leaves a partial file there.
laid() {
  local run=$1 name=$2 dir
  dir=$(mktemp -d)
  gh run download "$run" -n "$name" -D "$dir" || return 1
  mkdir "$dir/unpacked"
  zstd -d -c "$dir/stage.tar.zst" | tar -xf - -C "$dir/unpacked" || return 1
  mkdir -p "$cache" "$stage"
  # `cp` gives the copies new modification times, so every restored file is newer than the job's
  # mark and `used` keeps it.
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
    # Deletes each store directory unless a run since the mark used it. A store's last use is the
    # newer of the modification time of its `.used` file and of the directory itself. A run that
    # adds an entry to the directory updates the directory's time. The function `last_use` in
    # `sweep` computes the last use the same way.
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
          # Deletes these files one at a time when they are not newer than the mark, as `BY_FILE` in
          # `sweep` does. Each file is an independent executable or cached result.
          answered | reused) find "$dir" -type f ! -newer "$mark" -delete ;;
          # Deletes a stage or memo store unless some file in it is newer than the mark. For a stage
          # that file is its `.used`; for a store it is the directory itself, whose time changes
          # when each run creates a directory inside it.
          *) [ -n "$(find "$dir" -newer "$mark" -print -quit)" ] || rm -rf "$dir" ;;
        esac
      done
    fi
    # Deletes loose files directly in the stage directory that this run did not read or write. They
    # do not belong to any stage.
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
