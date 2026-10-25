#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
SCRIPT_SOURCE="$SCRIPT_DIR/orgbot-trash.py"
SERVICE_SOURCE="$SCRIPT_DIR/orgbot-trash-cleanup.service"
TIMER_SOURCE="$SCRIPT_DIR/orgbot-trash-cleanup.timer"
BIN_DIR="$HOME/.local/bin"
LIB_DIR="$HOME/.local/lib/orgbot"
UNIT_DIR="$HOME/.config/systemd/user"

for command in python3 systemctl mktemp cmp ln; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'Required command not found: %s\n' "$command" >&2
    exit 2
  }
done

for directory in "$HOME/.local" "$BIN_DIR" "$HOME/.local/lib" "$LIB_DIR" \
                 "$HOME/.config" "$HOME/.config/systemd" "$UNIT_DIR"; do
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

check_target() {
  local source=$1 target=$2 mode=$3
  if [[ -e "$target" || -L "$target" ]]; then
    if [[ -f "$target" && ! -L "$target" && $(stat -c '%u' -- "$target") == "$(id -u)" \
          && $(stat -c '%a' -- "$target") == "$mode" ]] && cmp -s -- "$source" "$target"; then
      return
    fi
    printf 'Refusing to overwrite existing target: %s\n' "$target" >&2
    exit 2
  fi
}

install_if_absent_or_identical() {
  local source=$1 target=$2 mode=$3 parent temporary
  parent=$(dirname -- "$target")
  if [[ -e "$target" || -L "$target" ]]; then
    if [[ -f "$target" && ! -L "$target" ]] && cmp -s -- "$source" "$target"; then
      return
    fi
    printf 'Refusing to overwrite existing target: %s\n' "$target" >&2
    exit 2
  fi
  temporary=$(mktemp "$parent/.orgbot-trash-install.XXXXXX")
  cp -- "$source" "$temporary"
  chmod "$mode" "$temporary"
  if ! ln -- "$temporary" "$target"; then
    rm -f -- "$temporary"
    printf 'Target appeared during installation; preserved it: %s\n' "$target" >&2
    exit 2
  fi
  rm -f -- "$temporary"
}

check_target "$SCRIPT_SOURCE" "$LIB_DIR/orgbot-trash.py" 755
check_target "$wrapper" "$BIN_DIR/orgbot-trash" 755
check_target "$SERVICE_SOURCE" "$UNIT_DIR/orgbot-trash-cleanup.service" 644
check_target "$TIMER_SOURCE" "$UNIT_DIR/orgbot-trash-cleanup.timer" 644

# Validate the mounted HDD and create its private trash root before installing
# a timer that can permanently clear expired entries.
python3 "$SCRIPT_SOURCE" list >/dev/null

install_if_absent_or_identical "$SCRIPT_SOURCE" "$LIB_DIR/orgbot-trash.py" 755
install_if_absent_or_identical "$wrapper" "$BIN_DIR/orgbot-trash" 755
install_if_absent_or_identical "$SERVICE_SOURCE" "$UNIT_DIR/orgbot-trash-cleanup.service" 644
install_if_absent_or_identical "$TIMER_SOURCE" "$UNIT_DIR/orgbot-trash-cleanup.timer" 644

systemctl --user daemon-reload
systemctl --user enable --now orgbot-trash-cleanup.timer
printf 'Installed %s and enabled orgbot-trash-cleanup.timer\n' "$BIN_DIR/orgbot-trash"
