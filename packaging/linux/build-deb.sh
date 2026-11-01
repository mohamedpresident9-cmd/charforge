#!/usr/bin/env bash
# Build a Debian package from the release binary.
# Usage: packaging/linux/build-deb.sh <version>   (run from the repository root)
set -euo pipefail

VERSION="${1:?usage: build-deb.sh <version>}"
ROOT="dist/deb"
BIN="target/release/charforge"

if [ ! -f "$BIN" ]; then
  echo "missing $BIN - build with 'cargo build --release' first" >&2
  exit 1
fi

rm -rf "$ROOT"
mkdir -p "$ROOT/DEBIAN" \
         "$ROOT/usr/bin" \
         "$ROOT/usr/share/applications" \
         "$ROOT/usr/share/doc/charforge"

install -m 0755 "$BIN" "$ROOT/usr/bin/charforge"

cat > "$ROOT/DEBIAN/control" <<EOF
Package: charforge
Version: ${VERSION}
Section: graphics
Priority: optional
Architecture: amd64
Maintainer: mohamedpresident9-cmd <mohamedpresident9-cmd@users.noreply.github.com>
Homepage: https://github.com/mohamedpresident9-cmd/charforge
Depends: libxkbcommon0, libx11-6, libgl1
Recommends: libwayland-client0
Description: Character studio with chunked asset transport
 CharForge Studio downloads character, animal, vehicle and building assets
 in verified chunks, keeps a local library and attaches animations and
 clothing to the current character.
EOF

cat > "$ROOT/usr/share/applications/charforge.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=CharForge Studio
Comment=Character studio with verified chunked downloads
Exec=charforge
Terminal=false
Categories=Graphics;3DGraphics;
EOF

cat > "$ROOT/usr/share/doc/charforge/copyright" <<EOF
CharForge Studio

Copyright (c) 2026 CharForge contributors
Licensed under the MIT license; see LICENSE in the source repository.
EOF

mkdir -p dist
# xz (not the runner default zstd) so the package installs on older Debian/Ubuntu too.
dpkg-deb -Zxz --build --root-owner-group "$ROOT" "dist/charforge_${VERSION}_amd64.deb"
echo "built dist/charforge_${VERSION}_amd64.deb"
