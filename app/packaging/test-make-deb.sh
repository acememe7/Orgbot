#!/usr/bin/env bash
set -euo pipefail

SOURCE=$(cd "$(dirname "$0")" && pwd)
VERSION=$(<"$SOURCE/version")
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
APP="$TMP/app"
mkdir -p "$APP/packaging" "$APP/assets" \
  "$APP/build/linux/x64/release/bundle/lib" "$TMP/bin"
mkdir "$TMP/staging"
cp "$SOURCE/make-deb.sh" "$APP/packaging/make-deb.sh"
cp "$SOURCE/orgbot.desktop" "$APP/packaging/orgbot.desktop"
cp "$SOURCE/version" "$APP/packaging/version"
cp "$SOURCE/../assets/logo.png" "$APP/assets/logo.png"
printf '#!/bin/sh\nexit 0\n' > "$APP/build/linux/x64/release/bundle/orgbot_app"
printf 'plugin\n' > "$APP/build/linux/x64/release/bundle/lib/libplugin.so"
chmod +x "$APP/build/linux/x64/release/bundle/orgbot_app"

cat > "$TMP/bin/dpkg-shlibdeps" <<'SHLIBDEPS'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$SHLIBDEPS_ARGS_FILE"
printf 'shlibs:Depends=libgtk-3-0t64, libsecret-1-0\n'
SHLIBDEPS
cat > "$TMP/bin/dpkg-deb" <<'DPKGDEB'
#!/usr/bin/env bash
cp "$3/DEBIAN/control" "$DEB_CONTROL_FILE"
: > "$4"
DPKGDEB
chmod +x "$TMP/bin/dpkg-shlibdeps" "$TMP/bin/dpkg-deb"
export PATH="$TMP/bin:$PATH"
export TMPDIR="$TMP/staging"
export SHLIBDEPS_ARGS_FILE="$TMP/shlibdeps-args"
export DEB_CONTROL_FILE="$TMP/control"

if ORGBOT_ENGINE="$TMP/missing-engine" "$APP/packaging/make-deb.sh" >"$TMP/out" 2>"$TMP/err"; then
  echo "packaging unexpectedly succeeded without an engine" >&2
  exit 1
fi
rg -q 'missing room engine' "$TMP/err"
test ! -e "$APP/orgbot_${VERSION}_amd64.deb"
test -z "$(ls -A "$TMP/staging")"

printf '#!/bin/sh\nexit 0\n' > "$TMP/orgbot-engine"
chmod +x "$TMP/orgbot-engine"
ORGBOT_ENGINE="$TMP/orgbot-engine" "$APP/packaging/make-deb.sh" >/dev/null
rg -Fqx "Version: $VERSION" "$TMP/control"
rg -q '^Depends: libgtk-3-0t64, libsecret-1-0, libsecret-tools, ffmpeg, gstreamer1.0-libav, gstreamer1.0-plugins-good$' "$TMP/control"
rg -q '^Description: CLI and API agents working together in shared rooms$' "$TMP/control"
rg -q '^usr/lib/orgbot/orgbot-engine$' "$SHLIBDEPS_ARGS_FILE"
rg -q '^usr/lib/orgbot/lib/libplugin\.so$' "$SHLIBDEPS_ARGS_FILE"
rg -q '^usr/lib/orgbot/orgbot_app$' "$SHLIBDEPS_ARGS_FILE"
test -z "$(ls -A "$TMP/staging")"
ORGBOT_ENGINE="$TMP/orgbot-engine" "$APP/packaging/make-deb.sh" 2.0.0 >/dev/null
rg -q '^Version: 2\.0\.0$' "$TMP/control"
test -z "$(ls -A "$TMP/staging")"
echo "Linux .deb packaging checks passed"
