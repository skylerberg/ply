#!/usr/bin/env bash
set -euo pipefail
name=${1:?usage: artifact-run.sh NAME}
gh api "repos/$GITHUB_REPOSITORY/actions/artifacts?name=$name&per_page=10" \
  -q '[.artifacts[] | select(.expired == false and .workflow_run.head_repository_id == .workflow_run.repository_id)]
      | sort_by(.created_at) | last | .workflow_run.id // empty'
