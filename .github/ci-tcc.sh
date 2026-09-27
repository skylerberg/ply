#!/usr/bin/env bash
#
# tcc, the C compiler `Profile::Development` shells out to.
#
#   .github/ci-tcc.sh harvest   the .deb packages, into ./tcc, for the cache the jobs share
#   .github/ci-tcc.sh install   install it: those packages, or apt if the cache was evicted
#
# The tier keys every compiled object by the compiler and the flags it was given, so two jobs that
# installed different tccs would share no object at all, and a job that installed none falls back
# to `cc -O0`. That is why this is one place: the job that fills the cache and the jobs that
# install from it must name the same package.
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
    # The cache is an optimisation, not the source: apt still has it if the entry was evicted.
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
