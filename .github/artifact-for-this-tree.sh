#!/usr/bin/env bash
# Prints "<run id> <artifact name>" of a `<prefix>-<tree>` artifact this commit may reuse, or
# nothing: this tree's own, else a parent's when nothing since that parent can reach the build.
#
#   .github/artifact-for-this-tree.sh nextest-archive <tree>
set -euo pipefail
prefix="$1"
tree="$2"

run_for() {
  gh api "repos/$GITHUB_REPOSITORY/actions/artifacts?name=$1&per_page=10" \
    -q '[.artifacts[] | select(.expired == false)] | sort_by(.created_at) | last | .workflow_run.id // empty'
}

run=$(run_for "$prefix-$tree")
if [ -n "$run" ]; then
  echo "$run $prefix-$tree"
  exit 0
fi

# $1: 1 is the pull request's head, 0 is main.
from_parent() {
  local parent ptree moved count found
  parent=$(gh api "repos/$GITHUB_REPOSITORY/commits/$GITHUB_SHA" -q ".parents[$1].sha // empty")
  [ -n "$parent" ] || return 1
  moved=$(gh api "repos/$GITHUB_REPOSITORY/compare/$parent...$GITHUB_SHA")
  # The comparison truncates its file list at 300.
  count=$(printf '%s' "$moved" | jq '.files | length')
  [ "$count" -gt 0 ] && [ "$count" -lt 300 ] || return 1
  moved=$(printf '%s' "$moved" | jq -r '.files[].filename')
  # Paths that cannot reach the build. Not ci.yml (it defines the build) nor benches/ (it holds a
  # workspace member).
  if printf '%s\n' "$moved" | grep -qvE '\.md$|^docs/|^\.github/([a-z-]+\.sh$|actions/suite/|workflows/(bless|profile|run)\.yml$)'; then
    return 1
  fi
  ptree=$(gh api "repos/$GITHUB_REPOSITORY/git/commits/$parent" -q '.tree.sha')
  found=$(run_for "$prefix-$ptree")
  [ -n "$found" ] || return 1
  echo "$found $prefix-$ptree"
}

from_parent 1 && exit 0
from_parent 0 && exit 0
exit 0
