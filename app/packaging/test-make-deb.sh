#!/usr/bin/env bash
set -euo pipefail

SOURCE=$(cd "$(dirname "$0")" && pwd)
VERSION=$(<"$SOURCE/version")
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
APP="$TMP/app"
mkdir -p "$APP/packaging" "$APP/assets" \
  "$APP/build/linux/x64/release/bundle/lib" \
  "$APP/build/linux/x64/release/bundle/data/flutter_assets/assets/fonts" "$TMP/bin"
mkdir "$TMP/staging"
cp "$SOURCE/../../LICENSE" "$TMP/LICENSE"
cp "$SOURCE/make-deb.sh" "$APP/packaging/make-deb.sh"
cp "$SOURCE/orgbot.desktop" "$APP/packaging/orgbot.desktop"
cp "$SOURCE/version" "$APP/packaging/version"
cp "$SOURCE/../assets/logo.png" "$APP/assets/logo.png"
cp "$SOURCE/orgbots-icon-256.png" "$APP/packaging/orgbots-icon-256.png"
cp "$SOURCE/orgbot-trash.py" "$APP/packaging/orgbot-trash.py"
cp "$SOURCE/orgbots.preinst" "$APP/packaging/orgbots.preinst"
printf '#!/usr/bin/env python3\n' > "$APP/packaging/orgbot-workspace.py"
printf '#!/bin/sh\nexit 0\n' > "$APP/build/linux/x64/release/bundle/orgbot_app"
printf 'plugin\n' > "$APP/build/linux/x64/release/bundle/lib/libplugin.so"
printf 'third-party font license\n' > "$APP/build/linux/x64/release/bundle/data/flutter_assets/assets/fonts/OFL.txt"
chmod +x "$APP/build/linux/x64/release/bundle/orgbot_app"

cat > "$TMP/bin/dpkg-shlibdeps" <<'SHLIBDEPS'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$SHLIBDEPS_ARGS_FILE"
printf 'shlibs:Depends=libgtk-3-0t64, libsecret-1-0\n'
SHLIBDEPS
cat > "$TMP/bin/dpkg-deb" <<'DPKGDEB'
#!/usr/bin/env bash
if rg -q '^Package: orgbot$' "$3/DEBIAN/control"; then
  cp "$3/DEBIAN/control" "$DEB_TRANSITION_CONTROL_FILE"
  find "$3" -type f -printf '%P\n' | sort > "$DEB_TRANSITION_FILES_FILE"
  cp "$3/usr/share/doc/orgbot/copyright" "$DEB_TRANSITION_COPYRIGHT_FILE"
else
  cp "$3/DEBIAN/control" "$DEB_CONTROL_FILE"
  find "$3" -type l -printf '%P -> %l\n' | sort > "$DEB_LINKS_FILE"
  find "$3" -type f -printf '%P\n' | sort > "$DEB_FILES_FILE"
  find "$3" -type f -printf '%P %m\n' | sort > "$DEB_MODES_FILE"
  cp "$3/usr/share/doc/orgbots/copyright" "$DEB_COPYRIGHT_FILE"
fi
: > "$4"
DPKGDEB
chmod +x "$TMP/bin/dpkg-shlibdeps" "$TMP/bin/dpkg-deb"
export PATH="$TMP/bin:$PATH"
export TMPDIR="$TMP/staging"
export SHLIBDEPS_ARGS_FILE="$TMP/shlibdeps-args"
export DEB_CONTROL_FILE="$TMP/control"
export DEB_TRANSITION_CONTROL_FILE="$TMP/transitional-control"
export DEB_LINKS_FILE="$TMP/links"
export DEB_FILES_FILE="$TMP/files"
export DEB_MODES_FILE="$TMP/modes"
export DEB_COPYRIGHT_FILE="$TMP/copyright"
export DEB_TRANSITION_FILES_FILE="$TMP/transitional-files"
export DEB_TRANSITION_COPYRIGHT_FILE="$TMP/transitional-copyright"

if ORGBOT_ENGINE="$TMP/missing-engine" "$APP/packaging/make-deb.sh" >"$TMP/out" 2>"$TMP/err"; then
  echo "packaging unexpectedly succeeded without an engine" >&2
  exit 1
fi
rg -q 'missing room engine' "$TMP/err"
test ! -e "$APP/orgbots_${VERSION}_amd64.deb"
test -z "$(ls -A "$TMP/staging")"

printf '#!/bin/sh\nexit 0\n' > "$TMP/orgbot-engine"
chmod +x "$TMP/orgbot-engine"
mv "$APP/packaging/orgbot-trash.py" "$TMP/orgbot-trash.py.disabled"
if ORGBOT_ENGINE="$TMP/orgbot-engine" "$APP/packaging/make-deb.sh" >"$TMP/out" 2>"$TMP/err"; then
  echo "packaging unexpectedly succeeded without the HDD trash helper" >&2
  exit 1
fi
rg -q 'missing trusted HDD trash helper' "$TMP/err"
test -z "$(ls -A "$TMP/staging")"
mv "$TMP/orgbot-trash.py.disabled" "$APP/packaging/orgbot-trash.py"
ORGBOT_ENGINE="$TMP/orgbot-engine" "$APP/packaging/make-deb.sh" >/dev/null
rg -Fqx "Version: $VERSION" "$TMP/control"
rg -Fqx 'Package: orgbots' "$TMP/control"
rg -Fqx "Breaks: orgbot (<< $VERSION)" "$TMP/control"
rg -Fqx "Replaces: orgbot (<< $VERSION)" "$TMP/control"
rg -Fqx "Package: orgbot" "$TMP/transitional-control"
rg -Fqx "Version: $VERSION" "$TMP/transitional-control"
rg -Fqx 'Architecture: all' "$TMP/transitional-control"
rg -Fqx "Depends: orgbots (= $VERSION)" "$TMP/transitional-control"
rg -q '^Depends: libgtk-3-0t64, libsecret-1-0, python3, bubblewrap, libnotify-bin, libsecret-tools, ffmpeg, gstreamer1.0-libav, gstreamer1.0-plugins-good$' "$TMP/control"
rg -q '^Description: CLI and API agents working together in shared rooms$' "$TMP/control"
rg -q '^usr/lib/orgbot/orgbot-engine$' "$SHLIBDEPS_ARGS_FILE"
rg -q '^usr/lib/orgbot/lib/libplugin\.so$' "$SHLIBDEPS_ARGS_FILE"
rg -q '^usr/lib/orgbot/orgbot_app$' "$SHLIBDEPS_ARGS_FILE"
rg -Fqx 'usr/bin/orgbot -> /usr/lib/orgbot/orgbot_app' "$TMP/links"
rg -Fqx 'usr/bin/orgbots -> /usr/lib/orgbot/orgbot_app' "$TMP/links"
rg -Fqx 'usr/bin/orgbot-engine -> /usr/lib/orgbot/orgbot-engine' "$TMP/links"
rg -Fqx 'usr/bin/orgbot-trash -> /usr/lib/orgbot/orgbot-trash.py' "$TMP/links"
rg -Fqx 'usr/lib/orgbot/orgbot-workspace.py' "$TMP/files"
rg -Fqx 'usr/lib/orgbot/orgbot-trash.py' "$TMP/files"
rg -Fqx 'usr/lib/orgbot/orgbot-trash.py 755' "$TMP/modes"
rg -Fqx 'DEBIAN/preinst 755' "$TMP/modes"
rg -Fqx 'usr/share/doc/orgbots/copyright' "$TMP/files"
rg -Fqx 'usr/lib/orgbot/data/flutter_assets/assets/fonts/OFL.txt' "$TMP/files"
rg -Fqx 'usr/share/doc/orgbot/copyright' "$TMP/transitional-files"
rg -Fqx 'Copyright 2026 The AECE Studio' "$TMP/copyright"
rg -Fqx 'Source: https://github.com/acememe7/Orgbot' "$TMP/copyright"
rg -Fqx 'Upstream-Contact: https://github.com/acememe7/Orgbot/issues' "$TMP/copyright"
rg -Fq 'Apache License' "$TMP/copyright"
rg -Fqx 'Copyright 2026 The AECE Studio' "$TMP/transitional-copyright"
rg -Fqx 'Source: https://github.com/acememe7/Orgbot' "$TMP/transitional-copyright"
rg -Fq 'Apache License' "$TMP/transitional-copyright"
test -f "$APP/orgbots_${VERSION}_amd64.deb"
test -f "$APP/orgbot_${VERSION}_all.deb"
test -z "$(ls -A "$TMP/staging")"
ORGBOT_ENGINE="$TMP/orgbot-engine" "$APP/packaging/make-deb.sh" 2.0.0 >/dev/null
rg -q '^Version: 2\.0\.0$' "$TMP/control"
test -z "$(ls -A "$TMP/staging")"
python3 - "$SOURCE/orgbots.preinst" "$TMP/upgrade" <<'PY'
from pathlib import Path
import subprocess
import sys

source, base = map(Path, sys.argv[1:])
base.mkdir()
helper = base / 'usr/lib/orgbot/orgbot-trash.py'
helper.parent.mkdir(parents=True)
script = base / 'preinst'
script.write_text(source.read_text().replace('/usr/lib/orgbot', str(helper.parent))
                  .replace('/var/lib/orgbots', str(base / 'var/lib/orgbots')))
marker = base / 'var/lib/orgbots/legacy-trash-policy'

def upgrade():
    subprocess.run(['sh', str(script), 'upgrade'], check=True)

upgrade()
assert not marker.exists()
helper.write_text('TRASH_ROOT = Path("/mnt/hdd/OrgBots Trash")\nrequires_selection = True\n')
upgrade()
assert not marker.exists()
helper.write_text('TRASH_ROOT = Path("/mnt/hdd/OrgBots Trash")\n')
upgrade()
assert marker.is_file() and marker.stat().st_mode & 0o777 == 0o644
marker.write_text('existing policy\n')
upgrade()
assert marker.read_text() == 'existing policy\n'
PY
echo "Linux .deb package rename and compatibility checks passed"
