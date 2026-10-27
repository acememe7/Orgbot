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

PACKAGE=orgbots
APP=orgbot
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

WORKSPACE_HELPER=packaging/orgbot-workspace.py
if [ ! -f "$WORKSPACE_HELPER" ]; then
  echo "missing trusted workspace helper at $WORKSPACE_HELPER" >&2
  exit 1
fi
TRASH_HELPER=packaging/orgbot-trash.py
if [ ! -f "$TRASH_HELPER" ]; then
  echo "missing trusted HDD trash helper at $TRASH_HELPER" >&2
  exit 1
fi

write_package_copyright() {
  local destination=$1
  mkdir -p "$(dirname "$destination")"
  {
    printf '%s\n' \
      'Upstream-Name: OrgBots' \
      'Source: https://github.com/acememe7/Orgbot' \
      'Upstream-Contact: https://github.com/acememe7/Orgbot/issues' \
      ''
    cat ../LICENSE
  } > "$destination"
  chmod 0644 "$destination"
}

STAGE=$(mktemp -d)
TRANSITIONAL_STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE" "$TRANSITIONAL_STAGE"' EXIT
mkdir -p "$STAGE/usr/lib/$APP" "$STAGE/usr/bin" \
         "$STAGE/usr/share/applications" \
         "$STAGE/usr/share/icons/hicolor/256x256/apps" \
         "$STAGE/DEBIAN"

cp -r "$BUNDLE/." "$STAGE/usr/lib/$APP/"
chmod 755 "$STAGE/usr/lib/$APP/orgbot_app"

# The engine the app runs rooms on. Installed as orgbot-engine so it cannot be
# confused with the app's own /usr/bin/orgbot launcher, which is what
# findEngine() looks for first.
install -m755 "$ENGINE" "$STAGE/usr/lib/$APP/orgbot-engine"
install -m755 "$WORKSPACE_HELPER" "$STAGE/usr/lib/$APP/orgbot-workspace.py"
install -m755 "$TRASH_HELPER" "$STAGE/usr/lib/$APP/orgbot-trash.py"
install -m755 packaging/orgbots.preinst "$STAGE/DEBIAN/preinst"
ln -s "/usr/lib/$APP/orgbot-engine" "$STAGE/usr/bin/orgbot-engine"
ln -s "/usr/lib/$APP/orgbot-trash.py" "$STAGE/usr/bin/orgbot-trash"
install -m644 packaging/orgbot.desktop "$STAGE/usr/share/applications/orgbot.desktop"
install -m644 packaging/orgbots-icon-256.png "$STAGE/usr/share/icons/hicolor/256x256/apps/$APP.png"
write_package_copyright "$STAGE/usr/share/doc/$PACKAGE/copyright"
ln -s "/usr/lib/$APP/orgbot_app" "$STAGE/usr/bin/$APP"
ln -s "/usr/lib/$APP/orgbot_app" "$STAGE/usr/bin/$PACKAGE"

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
    && printf 'Source: %s\n\nPackage: %s\nArchitecture: amd64\n' "$PACKAGE" "$PACKAGE" > debian/control )
# Scan the app, engine, and every bundled shared object: dynamically loaded
# Flutter plugins do not appear in the app executable's DT_NEEDED entries.
SHLIBDEPS_BINS=("usr/lib/$APP/orgbot_app" "usr/lib/$APP/orgbot-engine")
while IFS= read -r -d '' library; do
  SHLIBDEPS_BINS+=("${library#"$STAGE/"}")
done < <(find "$STAGE/usr/lib/$APP" -type f \
  \( -name '*.so' -o -name '*.so.*' \) -print0)
DEPENDS=$(cd "$STAGE" && dpkg-shlibdeps \
  -l"$STAGE/usr/lib/$APP/lib" -O "${SHLIBDEPS_BINS[@]}" \
  | sed -n 's/^shlibs:Depends=//p')
rm -rf "$STAGE/debian"
if [ -z "$DEPENDS" ]; then
  echo "dpkg-shlibdeps did not report package dependencies." >&2
  exit 1
fi

cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: $PACKAGE
Version: $VERSION
Section: devel
Priority: optional
Architecture: amd64
Breaks: orgbot (<< $VERSION)
Replaces: orgbot (<< $VERSION)
Installed-Size: $INSTALLED_KB
Depends: $DEPENDS, python3, bubblewrap, dbus-daemon, gnome-keyring, libnotify-bin, libsecret-tools, ffmpeg, gstreamer1.0-libav, gstreamer1.0-plugins-good
Maintainer: OrgBot <theaeceme@gmail.com>
Description: CLI and API agents working together in shared rooms
 Bots hold different roles, pick up tasks, hand work off to one another, wait
 on those handoffs, get their work inspected, and submit it - in one room you
 can watch and talk to. Codex, Claude, and Grok use their existing CLI sessions.
 OpenCode uses a selected provider API credential; Antigravity uses a separate
 private sign-in. DeepSeek and Ollama Cloud use securely saved API keys. Agents
 work together in the room's shared project folder with role permissions.
CONTROL

dpkg-deb --build --root-owner-group "$STAGE" "${PACKAGE}_${VERSION}_amd64.deb" >/dev/null

mkdir -p "$TRANSITIONAL_STAGE/DEBIAN"
write_package_copyright "$TRANSITIONAL_STAGE/usr/share/doc/$APP/copyright"
cat > "$TRANSITIONAL_STAGE/DEBIAN/control" <<TRANSITIONAL
Package: orgbot
Version: $VERSION
Section: oldlibs
Priority: optional
Architecture: all
Depends: orgbots (= $VERSION)
Maintainer: OrgBot <theaeceme@gmail.com>
Description: transitional package for OrgBots
 This package upgrades installations of the former orgbot package to orgbots.
TRANSITIONAL
dpkg-deb --build --root-owner-group "$TRANSITIONAL_STAGE" "orgbot_${VERSION}_all.deb" >/dev/null
echo "built ${PACKAGE}_${VERSION}_amd64.deb and orgbot_${VERSION}_all.deb"
