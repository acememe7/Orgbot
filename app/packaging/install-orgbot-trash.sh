#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
SCRIPT_SOURCE="$SCRIPT_DIR/orgbot-trash.py"
SERVICE_SOURCE="$SCRIPT_DIR/orgbot-trash-cleanup.service"
TIMER_SOURCE="$SCRIPT_DIR/orgbot-trash-cleanup.timer"
BIN_DIR="$HOME/.local/bin"
LIB_DIR="$HOME/.local/lib/orgbot"
CONFIG_HOME=${XDG_CONFIG_HOME:-"$HOME/.config"}
if [[ "$CONFIG_HOME" != /* ]]; then CONFIG_HOME="$HOME/.config"; fi
UNIT_DIR="$CONFIG_HOME/systemd/user"

for command in python3 systemctl mktemp cmp ln cp mv grep; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'Required command not found: %s\n' "$command" >&2
    exit 2
  }
done

for directory in "$HOME/.local" "$BIN_DIR" "$HOME/.local/lib" "$LIB_DIR" \
                 "$CONFIG_HOME" "$CONFIG_HOME/systemd" "$UNIT_DIR"; do
  if [[ -L "$directory" ]]; then
    printf 'Refusing symlink installation directory: %s\n' "$directory" >&2
    exit 2
  fi
  mkdir -p -- "$directory"
  if [[ ! -d "$directory" || $(stat -c '%u' -- "$directory") != "$(id -u)" ]]; then
    printf 'Installation directory must be owned by this user: %s\n' "$directory" >&2
    exit 2
  fi
done

wrapper=$(mktemp)
trap 'rm -f -- "$wrapper"' EXIT
cat >"$wrapper" <<'EOF'
#!/usr/bin/env bash
exec /usr/bin/python3 "$HOME/.local/lib/orgbot/orgbot-trash.py" "$@"
EOF
chmod 755 "$wrapper"

managed_existing_target() {
  local target=$1 kind=$2
  case "$kind" in
    helper) grep -Fq 'Recoverable, project-grouped file trash' "$target" ;;
    wrapper) grep -Fq '$HOME/.local/lib/orgbot/orgbot-trash.py' "$target" ;;
    service) grep -Fq 'Clear expired OrgBots recoverable trash entries' "$target" ;;
    timer) grep -Fq 'Daily cleanup of expired OrgBots trash entries' "$target" ;;
  esac
}

check_target() {
  local source=$1 target=$2 mode=$3 kind=$4
  if [[ -e "$target" || -L "$target" ]]; then
    if [[ -f "$target" && ! -L "$target" && $(stat -c '%u' -- "$target") == "$(id -u)" \
          && $(stat -c '%a' -- "$target") == "$mode" ]]; then
      if cmp -s -- "$source" "$target" || managed_existing_target "$target" "$kind"; then return; fi
    fi
    printf 'Refusing to overwrite existing target: %s\n' "$target" >&2
    exit 2
  fi
}

backup_dir=""
backup_existing() {
  local target=$1 name
  [[ -n "$backup_dir" ]] || { backup_dir=$(mktemp -d "$LIB_DIR/.trash-upgrade-backup.XXXXXX"); chmod 700 "$backup_dir"; }
  name=$(basename -- "$target")
  cp -p -- "$target" "$backup_dir/$name"
}

install_or_update() {
  local source=$1 target=$2 mode=$3 kind=$4 parent temporary
  parent=$(dirname -- "$target")
  if [[ -e "$target" || -L "$target" ]]; then
    cmp -s -- "$source" "$target" && return
    managed_existing_target "$target" "$kind" || { printf 'Refusing to overwrite existing target: %s\n' "$target" >&2; exit 2; }
    backup_existing "$target"
  fi
  temporary=$(mktemp "$parent/.orgbot-trash-install.XXXXXX")
  cp -- "$source" "$temporary"
  chmod "$mode" "$temporary"
  mv -f -- "$temporary" "$target"
}

check_target "$SCRIPT_SOURCE" "$LIB_DIR/orgbot-trash.py" 755 helper
check_target "$wrapper" "$BIN_DIR/orgbot-trash" 755 wrapper
check_target "$SERVICE_SOURCE" "$UNIT_DIR/orgbot-trash-cleanup.service" 644 service
check_target "$TIMER_SOURCE" "$UNIT_DIR/orgbot-trash-cleanup.timer" 644 timer

# Resolve and persist the selected root before replacing the old helper. This
# preserves legacy-HDD selection and prevents the newly installed helper from
# mistaking itself for evidence of an older HDD-only installation.
config_json=$(python3 "$SCRIPT_SOURCE" config)
readarray -d '' -t selected_values < <(python3 -c 'import json,sys; c=json.load(sys.stdin); sys.stdout.buffer.write(c["root"].encode()+b"\0"+(b"1" if c["requires_selection"] else b"0")+b"\0")' <<<"$config_json")
selected_root=${selected_values[0]}
if [[ ${selected_values[1]} != 0 ]]; then
  printf 'Trash location requires explicit owner selection before installation.\n' >&2
  exit 2
fi
python3 "$SCRIPT_SOURCE" configure --root "$selected_root" --defer-systemd >/dev/null
python3 "$SCRIPT_SOURCE" list >/dev/null

install_or_update "$SCRIPT_SOURCE" "$LIB_DIR/orgbot-trash.py" 755 helper
install_or_update "$wrapper" "$BIN_DIR/orgbot-trash" 755 wrapper
install_or_update "$SERVICE_SOURCE" "$UNIT_DIR/orgbot-trash-cleanup.service" 644 service
install_or_update "$TIMER_SOURCE" "$UNIT_DIR/orgbot-trash-cleanup.timer" 644 timer

python3 "$LIB_DIR/orgbot-trash.py" sync-systemd
systemctl --user enable --now orgbot-trash-cleanup.timer
printf 'Installed %s and enabled orgbot-trash-cleanup.timer\n' "$BIN_DIR/orgbot-trash"
if [[ -n "$backup_dir" ]]; then printf 'Previous managed files retained in %s\n' "$backup_dir"; fi
