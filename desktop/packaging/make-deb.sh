#!/usr/bin/env bash
# Package the desktop release binary as a .deb without the tauri bundler.
# Requires the release build (cargo build --release in src-tauri).
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=orgbot-desktop
VERSION=${1:-0.1.0}
BIN=src-tauri/target/release/gptbots-desktop
[ -x "$BIN" ] || { echo "missing $BIN - build first: (cd src-tauri && cargo build --release)"; exit 1; }

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/usr/lib/$NAME" "$STAGE/usr/bin" \
         "$STAGE/usr/share/applications" "$STAGE/usr/share/icons/hicolor/256x256/apps" \
         "$STAGE/DEBIAN"

install -m755 "$BIN" "$STAGE/usr/lib/$NAME/$NAME"
install -m644 packaging/orgbot.desktop "$STAGE/usr/share/applications/"
install -m644 src-tauri/icons/icon.png "$STAGE/usr/share/icons/hicolor/256x256/apps/orgbot.png"
ln -s "/usr/lib/$NAME/$NAME" "$STAGE/usr/bin/$NAME"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: $NAME
Version: $VERSION
Section: devel
Priority: optional
Architecture: amd64
Depends: libwebkit2gtk-4.1-0, libgtk-3-0, libayatana-appindicator3-1, librsvg2-common
Maintainer: GPTBots <theaeceme@gmail.com>
Description: Rooms of AI agents working a shared task board
 GPTBots hosts a room of AI bots (codex, claude, grok, or API-backed) that
 plan, hand off, inspect and submit work on a shared task board inside your
 project folders. This desktop shell hosts a room locally and shows its
 chat, roster and board.
EOF

dpkg-deb --build --root-owner-group "$STAGE" "${NAME}_${VERSION}_amd64.deb" 2>/dev/null || \
  dpkg-deb --build "$STAGE" "${NAME}_${VERSION}_amd64.deb"
echo "built ${NAME}_${VERSION}_amd64.deb"
