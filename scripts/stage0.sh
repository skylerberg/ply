#!/usr/bin/env bash
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
gh release download main-latest --repo skylerberg/ply --pattern "$asset" --output "$dest.download" --clobber
chmod +x "$dest.download"
mv -f "$dest.download" "$dest"
echo "$dest"
