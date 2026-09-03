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
VERSION=${1:-1.0.0}
BUNDLE=build/linux/x64/release/bundle

if [ ! -x "$BUNDLE/orgbot_app" ]; then
  echo "missing $BUNDLE/orgbot_app — build first:" >&2
  echo "  flutter build linux --release" >&2
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
ENGINE=${ORGBOT_ENGINE:-../core/target/release/orgbot}
if [ -x "$ENGINE" ]; then
  install -m755 "$ENGINE" "$STAGE/usr/lib/$NAME/orgbot-engine"
  ln -s "/usr/lib/$NAME/orgbot-engine" "$STAGE/usr/bin/orgbot-engine"
else
  echo "warning: engine not found at $ENGINE — rooms will not run." >&2
  echo "  build it with: (cd ../core && cargo build --release)" >&2
fi
install -m644 packaging/orgbot.desktop "$STAGE/usr/share/applications/orgbot.desktop"
install -m644 assets/logo.png "$STAGE/usr/share/icons/hicolor/256x256/apps/$NAME.png"
ln -s "/usr/lib/$NAME/orgbot_app" "$STAGE/usr/bin/$NAME"

INSTALLED_KB=$(du -sk "$STAGE/usr" | cut -f1)

# Work out the runtime dependencies from the binary itself rather than writing
# a list by hand. A hand-written list gets the names wrong across releases —
# Ubuntu 24.04 renamed libgtk-3-0 to libgtk-3-0t64 for the 64-bit time_t
# transition, so a hardcoded "libgtk-3-0" refuses to install there — and it
# quietly omits transitive libraries such as pango and harfbuzz.
DEPENDS="libgtk-3-0t64 | libgtk-3-0, libglib2.0-0t64 | libglib2.0-0, zlib1g"
if command -v dpkg-shlibdeps >/dev/null 2>&1; then
  ( cd "$STAGE" && mkdir -p debian \
      && printf 'Source: %s\n\nPackage: %s\nArchitecture: amd64\n' "$NAME" "$NAME" > debian/control )
  # -l adds the bundled Flutter libraries to the search path; they ship inside
  # this package, so there is no distro package to depend on for them.
  COMPUTED=$(cd "$STAGE" && dpkg-shlibdeps -l"$STAGE/usr/lib/$NAME/lib" \
      --ignore-missing-info -O "usr/lib/$NAME/orgbot_app" 2>/dev/null \
      | sed 's/^shlibs:Depends=//')
  rm -rf "$STAGE/debian"
  [ -n "$COMPUTED" ] && DEPENDS="$COMPUTED"
fi

cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: $NAME
Version: $VERSION
Section: devel
Priority: optional
Architecture: amd64
Installed-Size: $INSTALLED_KB
Depends: $DEPENDS
Maintainer: OrgBot <theaeceme@gmail.com>
Description: A group chat of AI bots working a shared task board
 Bots hold different roles, pick up tasks, hand work off to one another, wait
 on those handoffs, get their work inspected, and submit it - in one room you
 can watch and talk to. Bots can be backed by Claude, DeepSeek, or any
 OpenAI-compatible endpoint, and a single room can mix providers.
CONTROL

dpkg-deb --build --root-owner-group "$STAGE" "${NAME}_${VERSION}_amd64.deb" >/dev/null
echo "built ${NAME}_${VERSION}_amd64.deb"
