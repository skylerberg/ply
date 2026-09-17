#!/usr/bin/env bash
# The run whose `<prefix>-<tree>` artifact this checkout may take, and the artifact's name, or
# nothing at all. A pull request's run uploads its build under its own tree, and a push to main
# takes it back rather than compiling the same sources twice.
#
#   .github/artifact-for-this-tree.sh nextest-archive <tree>
#
# A merge lands a tree nothing has built whenever main moved under the pull request, which is
# every second merge of a pair. Either parent's build can still stand for that tree: the pull
# request's when main moved under the branch, and main's own when the merge adds nothing a
# compiler reads -- a record, a bench, a workflow. Whichever is asked, what moved between that
# parent and this commit has to miss the compiler, so both are tried and the first that holds
# answers. Asking only the pull request's parent rebuilt a record-only merge from cold, at a
# hundred seconds for the binary and a hundred and fifty for the archive.
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

# Which parent to ask: 1 is the pull request's head, 0 is main. A plain push has only the one.
from_parent() {
  local parent ptree moved count found
  parent=$(gh api "repos/$GITHUB_REPOSITORY/commits/$GITHUB_SHA" -q ".parents[$1].sha // empty")
  [ -n "$parent" ] || return 1
  moved=$(gh api "repos/$GITHUB_REPOSITORY/compare/$parent...$GITHUB_SHA")
  # The comparison stops listing files at three hundred, and a list that stops is one this cannot
  # read, since the file that reaches the compiler is the one left off the end.
  count=$(printf '%s' "$moved" | jq '.files | length')
  [ "$count" -gt 0 ] && [ "$count" -lt 300 ] || return 1
  moved=$(printf '%s' "$moved" | jq -r '.files[].filename')
  if printf '%s\n' "$moved" | grep -qvE '^(docs/|benches/|\.github/|[^/]*\.md$)'; then
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
