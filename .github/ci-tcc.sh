#!/usr/bin/env bash
set -euo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "${1:-}" in
  harvest)
    sudo apt-get update -qq
    sudo apt-get install -y -qq --download-only tcc
    mkdir -p tcc
    cp /var/cache/apt/archives/*.deb tcc/
    ls tcc
    ;;
  install)
    if compgen -G 'tcc/*.deb' >/dev/null; then
      sudo dpkg -i tcc/*.deb
    else
      sudo apt-get update -qq
      sudo apt-get install -y -qq tcc
    fi
    tcc -v
    ;;
  *)
    echo "usage: ci-tcc.sh {harvest|install}" >&2
    exit 2
    ;;
esac
