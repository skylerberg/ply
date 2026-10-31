#!/usr/bin/env bash
# scripts/stage0.sh [DEST]: downloads the `ply` binary that CI last published from main for this
# machine, and writes it to DEST (`target/stage0/ply` by default). `ply bootstrap` runs on it in a
# checkout that has no `ply` yet. CI jobs that have not built a `ply` use it to run `ply work`,
# which reads `ply.work`.
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
