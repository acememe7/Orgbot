#!/usr/bin/env bash
set -euo pipefail

SOURCE_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
TEMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/orgbot-trash-installer-test.XXXXXX")
trap 'rm -rf -- "$TEMP_ROOT"' EXIT

STAGE="$TEMP_ROOT/source"
FAKE_BIN="$TEMP_ROOT/fake-bin"
mkdir -p -- "$STAGE" "$FAKE_BIN"
cp -- "$SOURCE_DIR/orgbot-trash.py" "$SOURCE_DIR/orgbot-trash-cleanup.service" \
  "$SOURCE_DIR/orgbot-trash-cleanup.timer" "$SOURCE_DIR/install-orgbot-trash.sh" "$STAGE/"

cat >"$FAKE_BIN/systemctl" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >>"$SYSTEMCTL_LOG"
if [[ ${1:-} == --user && ${2:-} == show-environment ]]; then
  [[ -n ${SYSTEMD_MANAGER_CONFIG:-} ]] && printf 'XDG_CONFIG_HOME=%s\n' "$SYSTEMD_MANAGER_CONFIG"
fi
EOF
chmod 755 "$FAKE_BIN/systemctl"

python3 - "$STAGE/orgbot-trash.py" "$FAKE_BIN/systemctl" <<'PY'
from pathlib import Path
import sys

helper = Path(sys.argv[1])
source = helper.read_text(encoding="utf-8")
old = 'SYSTEMCTL_CANDIDATES = (Path("/usr/bin/systemctl"), Path("/bin/systemctl"))'
new = f'SYSTEMCTL_CANDIDATES = (Path({sys.argv[2]!r}),)'
if old not in source:
    raise SystemExit("systemctl candidate fixture point changed")
helper.write_text(source.replace(old, new), encoding="utf-8")
PY

run_installer() {
  local home=$1 config=$2 data=$3 log=$4 manager_config=${5:-$2}
  env HOME="$home" XDG_CONFIG_HOME="$config" XDG_DATA_HOME="$data" \
    SYSTEMCTL_LOG="$log" SYSTEMD_MANAGER_CONFIG="$manager_config" PATH="$FAKE_BIN:$PATH" \
    "$STAGE/install-orgbot-trash.sh"
}

# Fresh install: a custom XDG config location and the local data default must
# produce a persisted selection and an exact-path timer sandbox before enable.
FRESH_HOME="$TEMP_ROOT/fresh-home"
FRESH_CONFIG="$TEMP_ROOT/fresh-config"
FRESH_DATA="$TEMP_ROOT/fresh-data"
FRESH_LOG="$TEMP_ROOT/fresh-systemctl.log"
mkdir -p -- "$FRESH_HOME"
run_installer "$FRESH_HOME" "$FRESH_CONFIG" "$FRESH_DATA" "$FRESH_LOG" >/dev/null
python3 - "$FRESH_CONFIG/orgbots/trash.json" "$FRESH_DATA/orgbots/trash" <<'PY'
import json
from pathlib import Path
import sys

config = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert config["root"] == sys.argv[2], config
PY
FRESH_DROPIN="$FRESH_CONFIG/systemd/user/orgbot-trash-cleanup.service.d/20-trash-root.conf"
grep -Fq 'Environment="XDG_CONFIG_HOME=' "$FRESH_DROPIN"
grep -Fq "ReadWritePaths=\"$FRESH_DATA/orgbots/trash\"" "$FRESH_DROPIN"
grep -Fq "ExecStart=/usr/bin/python3 \"$FRESH_HOME/.local/lib/orgbot/orgbot-trash.py\" cleanup" "$FRESH_DROPIN"
grep -Fq 'daemon-reload' "$FRESH_LOG"
grep -Fq 'enable --now orgbot-trash-cleanup.timer' "$FRESH_LOG"
[[ ! -e "$FRESH_HOME/.config/systemd/user/orgbot-trash-cleanup.service" ]]

# Refuse systemd sync if its search path differs from the installer XDG path.
MISMATCH_HOME="$TEMP_ROOT/mismatch-home"
MISMATCH_CONFIG="$TEMP_ROOT/mismatch-config"
MISMATCH_DATA="$TEMP_ROOT/mismatch-data"
MISMATCH_LOG="$TEMP_ROOT/mismatch-systemctl.log"
mkdir -p -- "$MISMATCH_HOME"
set +e
run_installer "$MISMATCH_HOME" "$MISMATCH_CONFIG" "$MISMATCH_DATA" "$MISMATCH_LOG" "$TEMP_ROOT/manager-config" >/dev/null 2>&1
mismatch_status=$?
set -e
[[ $mismatch_status -eq 2 ]]
[[ ! -e "$MISMATCH_CONFIG/systemd/user/orgbot-trash-cleanup.service.d/20-trash-root.conf" ]]
! grep -Fq 'enable --now orgbot-trash-cleanup.timer' "$MISMATCH_LOG"

# Managed legacy upgrade: retain an existing trash entry and keep byte-for-byte
# backups of the replaced managed helper and unit before the timer is reloaded.
LEGACY_HOME="$TEMP_ROOT/legacy-home"
LEGACY_CONFIG="$TEMP_ROOT/legacy-config"
LEGACY_DATA="$TEMP_ROOT/legacy-data"
LEGACY_ROOT="$TEMP_ROOT/legacy-volume/OrgBots Trash"
LEGACY_LOG="$TEMP_ROOT/legacy-systemctl.log"
LEGACY_SOURCE="$TEMP_ROOT/legacy-source.txt"
mkdir -p -- "$LEGACY_HOME" "$TEMP_ROOT/legacy-volume"
printf 'preserve me\n' >"$LEGACY_SOURCE"
env HOME="$LEGACY_HOME" XDG_CONFIG_HOME="$LEGACY_CONFIG" XDG_DATA_HOME="$LEGACY_DATA" \
  python3 "$STAGE/orgbot-trash.py" configure --root "$LEGACY_ROOT" >/dev/null
env HOME="$LEGACY_HOME" XDG_CONFIG_HOME="$LEGACY_CONFIG" XDG_DATA_HOME="$LEGACY_DATA" \
  python3 "$STAGE/orgbot-trash.py" move --project Legacy "$LEGACY_SOURCE" >/dev/null
LEGACY_HELPER="$LEGACY_HOME/.local/lib/orgbot/orgbot-trash.py"
LEGACY_UNIT_DIR="$LEGACY_CONFIG/systemd/user"
mkdir -p -- "$(dirname -- "$LEGACY_HELPER")" "$LEGACY_UNIT_DIR"
OLD_HELPER='#!/usr/bin/env python3
"""Recoverable, project-grouped file trash for OrgBots owners."""
TRASH_ROOT = Path("/mnt/hdd/OrgBots Trash")
'
printf '%s' "$OLD_HELPER" >"$LEGACY_HELPER"
chmod 755 "$LEGACY_HELPER"
cat >"$LEGACY_HOME/.local/bin-orgbot-trash-wrapper" <<'EOF'
#!/usr/bin/env bash
exec /usr/bin/python3 "$HOME/.local/lib/orgbot/orgbot-trash.py" "$@"
EOF
mkdir -p -- "$LEGACY_HOME/.local/bin"
mv -- "$LEGACY_HOME/.local/bin-orgbot-trash-wrapper" "$LEGACY_HOME/.local/bin/orgbot-trash"
chmod 755 "$LEGACY_HOME/.local/bin/orgbot-trash"
cat >"$LEGACY_UNIT_DIR/orgbot-trash-cleanup.service" <<EOF
[Unit]
Description=Clear expired OrgBots recoverable trash entries
[Service]
ExecStart=/usr/bin/python3 %h/.local/lib/orgbot/orgbot-trash.py cleanup
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths="/mnt/hdd/OrgBots Trash"
EOF
cat >"$LEGACY_UNIT_DIR/orgbot-trash-cleanup.timer" <<'EOF'
[Unit]
Description=Daily cleanup of expired OrgBots trash entries
[Timer]
OnCalendar=daily
Persistent=true
Unit=orgbot-trash-cleanup.service
[Install]
WantedBy=timers.target
EOF
chmod 644 "$LEGACY_UNIT_DIR/orgbot-trash-cleanup.service" "$LEGACY_UNIT_DIR/orgbot-trash-cleanup.timer"
cp -- "$LEGACY_HELPER" "$TEMP_ROOT/old-helper.copy"
cp -- "$LEGACY_UNIT_DIR/orgbot-trash-cleanup.service" "$TEMP_ROOT/old-service.copy"
run_installer "$LEGACY_HOME" "$LEGACY_CONFIG" "$LEGACY_DATA" "$LEGACY_LOG" >/dev/null
BACKUP_DIR=$(find "$LEGACY_HOME/.local/lib/orgbot" -maxdepth 1 -type d -name '.trash-upgrade-backup.*' -print -quit)
[[ -n "$BACKUP_DIR" ]]
cmp -s "$TEMP_ROOT/old-helper.copy" "$BACKUP_DIR/orgbot-trash.py"
cmp -s "$TEMP_ROOT/old-service.copy" "$BACKUP_DIR/orgbot-trash-cleanup.service"
env HOME="$LEGACY_HOME" XDG_CONFIG_HOME="$LEGACY_CONFIG" XDG_DATA_HOME="$LEGACY_DATA" \
  python3 "$LEGACY_HELPER" list >"$TEMP_ROOT/legacy-list.jsonl"
[[ $(wc -l <"$TEMP_ROOT/legacy-list.jsonl") -eq 1 ]]
grep -Fq "ExecStart=/usr/bin/python3 \"$LEGACY_HELPER\" cleanup" \
  "$LEGACY_CONFIG/systemd/user/orgbot-trash-cleanup.service.d/20-trash-root.conf"

# Refuse an unrecognized existing helper without replacing or deleting it.
FOREIGN_HOME="$TEMP_ROOT/foreign-home"
FOREIGN_CONFIG="$TEMP_ROOT/foreign-config"
FOREIGN_DATA="$TEMP_ROOT/foreign-data"
FOREIGN_LOG="$TEMP_ROOT/foreign-systemctl.log"
FOREIGN_HELPER="$FOREIGN_HOME/.local/lib/orgbot/orgbot-trash.py"
mkdir -p -- "$(dirname -- "$FOREIGN_HELPER")"
printf 'owner file; not OrgBots managed\n' >"$FOREIGN_HELPER"
chmod 755 "$FOREIGN_HELPER"
cp -- "$FOREIGN_HELPER" "$TEMP_ROOT/foreign-helper.copy"
set +e
run_installer "$FOREIGN_HOME" "$FOREIGN_CONFIG" "$FOREIGN_DATA" "$FOREIGN_LOG" >/dev/null 2>&1
status=$?
set -e
[[ $status -eq 2 ]]
cmp -s "$TEMP_ROOT/foreign-helper.copy" "$FOREIGN_HELPER"
[[ ! -e "$FOREIGN_CONFIG/systemd/user/orgbot-trash-cleanup.service" ]]

printf 'installer fixture passed: fresh install, managed upgrade, foreign-file refusal\n'
