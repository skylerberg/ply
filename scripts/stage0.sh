#!/usr/bin/env bash
# scripts/stage0.sh [DEST]: the `ply` CI last published from main for this machine, written to DEST
# (`target/stage0/ply` by default): what `ply bootstrap` runs on in a checkout with no `ply` yet, and
# what reads `ply.work` (`ply work`) where a job has built none.
set -euo pipefail
dest=${1:-target/stage0/ply}
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) asset=ply-macos-arm64 ;;
  Linux-x86_64) asset=ply-linux-x64 ;;
  *)
    echo "stage0: no release is built for $(uname -s)-$(uname -m)" >&2
    exit 2
    ;;
esac
mkdir -p "$(dirname "$dest")"
# Written beside DEST and renamed over it: macOS kills a binary rewritten in place once it has run.
gh release download main-latest --repo skylerberg/ply --pattern "$asset" --output "$dest.download" --clobber
chmod +x "$dest.download"
mv -f "$dest.download" "$dest"
echo "$dest"
