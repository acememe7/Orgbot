#!/usr/bin/env bash
# Package the Flutter Linux app as a .deb.
#
# Flutter ships a *bundle* — the executable plus its lib/ and data/ — not a
# single binary, so the whole directory is installed under /usr/lib and a
# launcher is symlinked onto the path. The window icon is read at runtime from
# data/flutter_assets, which the bundle carries, so there is no second copy of
# the logo to keep in step.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=orgbot
VERSION=${1:-$(<packaging/version)}
BUNDLE=build/linux/x64/release/bundle

if [[ ! "$VERSION" =~ ^[0-9][A-Za-z0-9.+:~-]*$ ]]; then
  echo "invalid Debian package version: $VERSION" >&2
  exit 1
fi

if [ ! -x "$BUNDLE/orgbot_app" ]; then
  echo "missing $BUNDLE/orgbot_app — build first:" >&2
  echo "  flutter build linux --release" >&2
  exit 1
fi

# The room engine is required at runtime; reject an incomplete package before
# allocating or copying the package staging tree.
ENGINE=${ORGBOT_ENGINE:-../core/target/release/orgbot}
if [ ! -x "$ENGINE" ]; then
  echo "missing room engine at $ENGINE — build it first:" >&2
  echo "  (cd ../core && cargo build --release)" >&2
  exit 1
fi

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/usr/lib/$NAME" "$STAGE/usr/bin" \
         "$STAGE/usr/share/applications" \
         "$STAGE/usr/share/icons/hicolor/256x256/apps" \
         "$STAGE/DEBIAN"

cp -r "$BUNDLE/." "$STAGE/usr/lib/$NAME/"
chmod 755 "$STAGE/usr/lib/$NAME/orgbot_app"

# The engine the app runs rooms on. Installed as orgbot-engine so it cannot be
# confused with the app's own /usr/bin/orgbot launcher, which is what
# findEngine() looks for first.
install -m755 "$ENGINE" "$STAGE/usr/lib/$NAME/orgbot-engine"
ln -s "/usr/lib/$NAME/orgbot-engine" "$STAGE/usr/bin/orgbot-engine"
install -m644 packaging/orgbot.desktop "$STAGE/usr/share/applications/orgbot.desktop"
install -m644 packaging/orgbots-icon-256.png "$STAGE/usr/share/icons/hicolor/256x256/apps/$NAME.png"
ln -s "/usr/lib/$NAME/orgbot_app" "$STAGE/usr/bin/$NAME"

INSTALLED_KB=$(du -sk "$STAGE/usr" | cut -f1)

# Work out the runtime dependencies from the binary itself rather than writing
# a list by hand. A hand-written list gets the names wrong across releases —
# Ubuntu 24.04 renamed libgtk-3-0 to libgtk-3-0t64 for the 64-bit time_t
# transition, so a hardcoded "libgtk-3-0" refuses to install there — and it
# quietly omits transitive libraries such as pango and harfbuzz.
command -v dpkg-shlibdeps >/dev/null 2>&1 || {
  echo "dpkg-shlibdeps is required to calculate package dependencies (install dpkg-dev)." >&2
  exit 1
}
( cd "$STAGE" && mkdir -p debian \
    && printf 'Source: %s\n\nPackage: %s\nArchitecture: amd64\n' "$NAME" "$NAME" > debian/control )
# Scan the app, engine, and every bundled shared object: dynamically loaded
# Flutter plugins do not appear in the app executable's DT_NEEDED entries.
SHLIBDEPS_BINS=("usr/lib/$NAME/orgbot_app" "usr/lib/$NAME/orgbot-engine")
while IFS= read -r -d '' library; do
  SHLIBDEPS_BINS+=("${library#"$STAGE/"}")
done < <(find "$STAGE/usr/lib/$NAME" -type f \
  \( -name '*.so' -o -name '*.so.*' \) -print0)
DEPENDS=$(cd "$STAGE" && dpkg-shlibdeps \
  -l"$STAGE/usr/lib/$NAME/lib" -O "${SHLIBDEPS_BINS[@]}" \
  | sed -n 's/^shlibs:Depends=//p')
rm -rf "$STAGE/debian"
if [ -z "$DEPENDS" ]; then
  echo "dpkg-shlibdeps did not report package dependencies." >&2
  exit 1
fi

cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: $NAME
Version: $VERSION
Section: devel
Priority: optional
Architecture: amd64
Installed-Size: $INSTALLED_KB
Depends: $DEPENDS, libsecret-tools, ffmpeg, gstreamer1.0-libav, gstreamer1.0-plugins-good
Maintainer: OrgBot <theaeceme@gmail.com>
Description: CLI and API agents working together in shared rooms
 Bots hold different roles, pick up tasks, hand work off to one another, wait
 on those handoffs, get their work inspected, and submit it - in one room you
 can watch and talk to. Codex, Claude, and Grok CLIs use their existing local
 sessions. DeepSeek and Ollama Cloud use securely saved API keys. Agents
 work together in the room's shared project folder with role permissions.
CONTROL

dpkg-deb --build --root-owner-group "$STAGE" "${NAME}_${VERSION}_amd64.deb" >/dev/null
echo "built ${NAME}_${VERSION}_amd64.deb"
