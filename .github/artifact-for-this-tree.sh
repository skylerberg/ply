#!/usr/bin/env bash
# The run whose `<prefix>-<tree>` artifact this checkout may take, and the artifact's name, or
# nothing at all. A pull request's run uploads its build under its own tree, and a push to main
# takes it back rather than compiling the same sources twice.
#
#   .github/artifact-for-this-tree.sh nextest-archive <tree>
#
# A merge lands a tree nothing has built whenever main moved under the pull request, which is
# every second merge of a pair. The pull request's own build still stands there, so long as what
# moved since cannot reach a compiler: records, benches and the workflows themselves.
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

parent=$(gh api "repos/$GITHUB_REPOSITORY/commits/$GITHUB_SHA" -q '.parents[1].sha // empty')
[ -n "$parent" ] || exit 0
moved=$(gh api "repos/$GITHUB_REPOSITORY/compare/$parent...$GITHUB_SHA")
# The comparison stops listing files at three hundred, and a list that stops is one this cannot
# read, since the file that reaches the compiler is the one left off the end.
count=$(printf '%s' "$moved" | jq '.files | length')
[ "$count" -gt 0 ] && [ "$count" -lt 300 ] || exit 0
moved=$(printf '%s' "$moved" | jq -r '.files[].filename')
if printf '%s\n' "$moved" | grep -qvE '^(docs/|benches/|\.github/|[^/]*\.md$)'; then
  exit 0
fi
ptree=$(gh api "repos/$GITHUB_REPOSITORY/git/commits/$parent" -q '.tree.sha')
run=$(run_for "$prefix-$ptree")
[ -n "$run" ] || exit 0
echo "$run $prefix-$ptree"
