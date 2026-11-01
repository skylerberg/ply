#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
for tool in actionlint shellcheck gawk mawk; do
  command -v "$tool" > /dev/null || { echo "ci-lint: no $tool on this machine: brew install $tool" >&2; exit 2; }
done
actionlint
git ls-files -z '*.sh' | xargs -0 shellcheck --severity=warning
shim=$(mktemp -d)
trap 'rm -rf "$shim"' EXIT
for awk in awk gawk mawk; do
  ln -sf "$(command -v "$awk")" "$shim/awk"
  PATH="$shim:$PATH" .github/ci-shards.sh verify > /dev/null
  PATH="$shim:$PATH" .github/ci-timings.sh --self-test > /dev/null
  echo "under $awk: the CI tables are consistent and the timings tool passes its self-test"
done
echo "actionlint and shellcheck found nothing"
